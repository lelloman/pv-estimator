#!/usr/bin/env python3
"""Train weather quantile MLPs with PyTorch/CUDA for the ML weather experiment."""

from __future__ import annotations

import argparse
import csv
import gzip
import hashlib
import json
import math
import time
from pathlib import Path

import numpy as np
import torch
from torch import nn
from torch.utils.data import DataLoader, TensorDataset

BASE_INPUTS = 64
TARGETS = 5
QUANTILES = torch.tensor([0.1, 0.5, 0.9], dtype=torch.float32)
TARGET_NAMES = [
    "ghi_w_m2",
    "dni_w_m2",
    "dhi_w_m2",
    "ambient_temperature_c",
    "wind_speed_m_s",
]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--data", type=Path, default=Path("data/nasa_power_hourly.csv.gz"))
    parser.add_argument("--out-dir", type=Path, default=Path("results/weather_mlp_100k_torch"))
    parser.add_argument("--train-limit", type=int, default=1_000_000)
    parser.add_argument("--val-limit", type=int, default=100_000)
    parser.add_argument("--train-stride", type=int, default=13)
    parser.add_argument("--val-stride", type=int, default=7)
    parser.add_argument("--hidden-width", type=int, default=256)
    parser.add_argument("--hidden-layers", default=None, help="comma-separated hidden layer widths, e.g. 256,256,128")
    parser.add_argument("--architecture", choices=["mlp", "residual-mlp"], default="mlp")
    parser.add_argument("--residual-width", type=int, default=512)
    parser.add_argument("--residual-blocks", type=int, default=6)
    parser.add_argument("--residual-scale", type=float, default=0.5)
    parser.add_argument("--epochs", type=int, default=20)
    parser.add_argument("--batch-size", type=int, default=8192)
    parser.add_argument("--learning-rate", type=float, default=1e-3)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--cache-dir", type=Path, default=None)
    parser.add_argument("--input-cache", type=Path, default=None, help="explicit cached tensor .npz to load instead of resolving cache-dir")
    parser.add_argument("--zero-aux-width", type=int, default=0, help="append this many zero-valued auxiliary features to an explicit input cache")
    parser.add_argument("--aux-features", type=Path, default=None, help="location-level auxiliary feature CSV keyed by location_id")
    parser.add_argument("--aux-include-prefixes", default=None, help="comma-separated aux column prefixes to keep, e.g. etopo_,cci_")
    parser.add_argument("--aux-clip-abs", type=float, default=None, help="clip auxiliary input columns to +/- this value after normalization")
    parser.add_argument(
        "--aux-exclude-columns",
        default="location_id,name,latitude,longitude,region,cci_point_class",
        help="comma-separated auxiliary CSV columns to exclude from model inputs",
    )
    parser.add_argument("--rebuild-cache", action="store_true")
    parser.add_argument("--resident-device", action="store_true")
    parser.add_argument(
        "--sample-mode",
        choices=["prefix", "reservoir"],
        default="prefix",
        help="prefix is fast for small/full-covered datasets; reservoir samples across the whole CSV",
    )
    return parser.parse_args()


def parse_hidden_layers(args: argparse.Namespace) -> list[int]:
    if args.hidden_layers is None:
        return [args.hidden_width, args.hidden_width]
    layers = [int(value.strip()) for value in args.hidden_layers.split(",") if value.strip()]
    if not layers or any(width <= 0 for width in layers):
        raise ValueError("--hidden-layers must contain positive integer widths")
    return layers


def main() -> int:
    args = parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)
    hidden_layers = parse_hidden_layers(args)
    validate_architecture_args(args)
    validate_feature_args(args)
    torch.manual_seed(args.seed)
    np.random.seed(args.seed)

    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    print(f"device={device}")
    if device.type == "cuda":
        print(f"gpu={torch.cuda.get_device_name(0)}")

    aux_features = load_aux_features(args.aux_features, args.aux_exclude_columns, args.aux_include_prefixes)
    input_features = BASE_INPUTS + aux_features.width + args.zero_aux_width
    print(
        f"input_features={input_features} base_features={BASE_INPUTS} "
        f"aux_features={aux_features.width} zero_aux_width={args.zero_aux_width}"
    )

    started = time.time()
    train_x, train_y, val_x, val_y = load_or_build_samples(args, aux_features, input_features)
    if train_x.shape[1] != input_features:
        if args.input_cache is None or aux_features.width or args.zero_aux_width:
            raise RuntimeError(f"loaded feature width {train_x.shape[1]} does not match expected {input_features}")
        input_features = int(train_x.shape[1])
        print(f"using explicit cache feature width={input_features}", flush=True)
    apply_aux_clip(train_x, val_x, args.aux_clip_abs)
    print(f"loaded train_rows={len(train_x)} val_rows={len(val_x)} in {time.time() - started:.1f}s")

    target_mean = train_y.mean(axis=0).astype(np.float32)
    target_std = train_y.std(axis=0).astype(np.float32)
    target_std = np.maximum(target_std, 1.0).astype(np.float32)
    train_y_norm = (train_y - target_mean) / target_std

    model = build_model(args, hidden_layers, input_features).to(device)
    parameters = sum(parameter.numel() for parameter in model.parameters())
    print(f"parameters={parameters}")

    if args.resident_device:
        print("moving training tensors to device", flush=True)
        train_x_t = torch.from_numpy(train_x).to(device)
        train_y_t = torch.from_numpy(train_y_norm.astype(np.float32)).to(device)
        train_loader = None
    else:
        train_dataset = TensorDataset(
            torch.from_numpy(train_x),
            torch.from_numpy(train_y_norm.astype(np.float32)),
        )
        train_loader = DataLoader(
            train_dataset,
            batch_size=args.batch_size,
            shuffle=True,
            num_workers=0,
            pin_memory=device.type == "cuda",
        )
    val_x_t = torch.from_numpy(val_x).to(device)

    optimizer = torch.optim.AdamW(model.parameters(), lr=args.learning_rate)
    quantiles = QUANTILES.to(device)

    history: list[dict[str, object]] = []
    for epoch in range(1, args.epochs + 1):
        model.train()
        total_loss = 0.0
        batches = 0
        if args.resident_device:
            permutation = torch.randperm(train_x_t.shape[0], device=device)
            for start in range(0, train_x_t.shape[0], args.batch_size):
                batch_indices = permutation[start : start + args.batch_size]
                batch_x = train_x_t.index_select(0, batch_indices)
                batch_y = train_y_t.index_select(0, batch_indices)
                optimizer.zero_grad(set_to_none=True)
                predictions = model(batch_x).view(-1, TARGETS, 3)
                loss = pinball_loss(predictions, batch_y, quantiles)
                loss.backward()
                optimizer.step()
                total_loss += float(loss.detach().cpu())
                batches += 1
        else:
            for batch_x, batch_y in train_loader:
                batch_x = batch_x.to(device, non_blocking=True)
                batch_y = batch_y.to(device, non_blocking=True)
                optimizer.zero_grad(set_to_none=True)
                predictions = model(batch_x).view(-1, TARGETS, 3)
                loss = pinball_loss(predictions, batch_y, quantiles)
                loss.backward()
                optimizer.step()
                total_loss += float(loss.detach().cpu())
                batches += 1

        metrics = evaluate(model, val_x_t, val_y, target_mean, target_std, quantiles, device)
        metrics["epoch"] = epoch
        metrics["train_pinball"] = total_loss / max(batches, 1)
        history.append(metrics)
        print(
            f"epoch={epoch} train_pinball={metrics['train_pinball']:.6f} "
            f"val_pinball={metrics['pinball_loss']:.6f} val_mae={metrics['mae']}"
        )

    final_metrics = history[-1]
    output = {
        "model": "weather_mlp_torch",
        "architecture": args.architecture,
        "input_features": input_features,
        "base_input_features": BASE_INPUTS,
        "aux_features_path": str(args.aux_features) if args.aux_features else None,
        "aux_feature_count": aux_features.width,
        "aux_feature_columns": aux_features.columns,
        "aux_include_prefixes": args.aux_include_prefixes,
        "aux_clip_abs": args.aux_clip_abs,
        "zero_aux_width": args.zero_aux_width,
        "input_cache": str(args.input_cache) if args.input_cache else None,
        "hidden_width": args.hidden_width,
        "hidden_layers": hidden_layers,
        "residual_width": args.residual_width,
        "residual_blocks": args.residual_blocks,
        "residual_scale": args.residual_scale,
        "outputs": TARGETS * 3,
        "parameters": parameters,
        "epochs": args.epochs,
        "batch_size": args.batch_size,
        "learning_rate": args.learning_rate,
        "train_limit": args.train_limit,
        "val_limit": args.val_limit,
        "target_names": TARGET_NAMES,
        "target_mean": target_mean.tolist(),
        "target_std": target_std.tolist(),
        "device": str(device),
        "cache_dir": str(args.cache_dir) if args.cache_dir else None,
        "resident_device": args.resident_device,
        "sample_mode": args.sample_mode,
        "gpu": torch.cuda.get_device_name(0) if device.type == "cuda" else None,
        "history": history,
        **{key: value for key, value in final_metrics.items() if key != "epoch"},
    }
    metrics_path = args.out_dir / "metrics.json"
    metrics_path.write_text(json.dumps(output, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    torch.save(
        {
            "model_state_dict": model.state_dict(),
            "target_mean": target_mean,
            "target_std": target_std,
            "target_names": TARGET_NAMES,
            "architecture": args.architecture,
            "hidden_layers": hidden_layers,
            "residual_width": args.residual_width,
            "residual_blocks": args.residual_blocks,
            "residual_scale": args.residual_scale,
            "input_features": input_features,
            "base_input_features": BASE_INPUTS,
            "aux_features_path": str(args.aux_features) if args.aux_features else None,
            "aux_feature_count": aux_features.width,
            "aux_feature_columns": aux_features.columns,
            "aux_include_prefixes": args.aux_include_prefixes,
            "aux_clip_abs": args.aux_clip_abs,
            "zero_aux_width": args.zero_aux_width,
            "input_cache": str(args.input_cache) if args.input_cache else None,
        },
        args.out_dir / "model.pt",
    )
    print(f"wrote {metrics_path}")
    print(f"wrote {args.out_dir / 'model.pt'}")
    return 0


def validate_architecture_args(args: argparse.Namespace) -> None:
    if args.residual_width <= 0:
        raise ValueError("--residual-width must be positive")
    if args.residual_blocks <= 0:
        raise ValueError("--residual-blocks must be positive")
    if args.residual_scale <= 0:
        raise ValueError("--residual-scale must be positive")


def validate_feature_args(args: argparse.Namespace) -> None:
    if args.zero_aux_width < 0:
        raise ValueError("--zero-aux-width must be non-negative")
    if args.zero_aux_width and args.aux_features is not None:
        raise ValueError("--zero-aux-width and --aux-features are mutually exclusive")
    if args.aux_clip_abs is not None and args.aux_clip_abs <= 0:
        raise ValueError("--aux-clip-abs must be positive")
    if args.zero_aux_width and args.input_cache is None:
        raise ValueError("--zero-aux-width requires --input-cache")


def build_model(args: argparse.Namespace, hidden_layers: list[int], input_features: int) -> nn.Module:
    if args.architecture == "residual-mlp":
        return ResidualWeatherMlp(input_features, args.residual_width, args.residual_blocks, args.residual_scale)
    return WeatherMlp(input_features, hidden_layers)


class WeatherMlp(nn.Module):
    def __init__(self, input_features: int, hidden_layers: list[int]) -> None:
        super().__init__()
        layers: list[nn.Module] = []
        previous_width = input_features
        for hidden_width in hidden_layers:
            layers.append(nn.Linear(previous_width, hidden_width))
            layers.append(nn.SiLU())
            previous_width = hidden_width
        layers.append(nn.Linear(previous_width, TARGETS * 3))
        self.net = nn.Sequential(*layers)

    def forward(self, values: torch.Tensor) -> torch.Tensor:
        return self.net(values)


class ResidualBlock(nn.Module):
    def __init__(self, width: int, residual_scale: float) -> None:
        super().__init__()
        self.residual_scale = residual_scale
        self.net = nn.Sequential(
            nn.LayerNorm(width),
            nn.Linear(width, width),
            nn.SiLU(),
            nn.Linear(width, width),
        )

    def forward(self, values: torch.Tensor) -> torch.Tensor:
        return values + self.residual_scale * self.net(values)


class ResidualWeatherMlp(nn.Module):
    def __init__(self, input_features: int, width: int, blocks: int, residual_scale: float) -> None:
        super().__init__()
        self.input = nn.Linear(input_features, width)
        self.blocks = nn.Sequential(*(ResidualBlock(width, residual_scale) for _ in range(blocks)))
        self.output = nn.Sequential(
            nn.LayerNorm(width),
            nn.SiLU(),
            nn.Linear(width, TARGETS * 3),
        )

    def forward(self, values: torch.Tensor) -> torch.Tensor:
        hidden = self.input(values)
        hidden = self.blocks(hidden)
        return self.output(hidden)


def pinball_loss(predictions: torch.Tensor, targets: torch.Tensor, quantiles: torch.Tensor) -> torch.Tensor:
    errors = targets.unsqueeze(-1) - predictions
    losses = torch.maximum(quantiles * errors, (quantiles - 1.0) * errors)
    return losses.mean()


def load_or_build_samples(
    args: argparse.Namespace, aux_features: "AuxFeatureSet", input_features: int
) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    if args.input_cache is not None:
        return load_explicit_cache(args, input_features)

    if args.cache_dir is None:
        return load_samples(args, aux_features, input_features)

    args.cache_dir.mkdir(parents=True, exist_ok=True)
    cache_version = "v2" if aux_features.width == 0 else f"aux{aux_features.cache_key}_v3"
    cache_path = args.cache_dir / (
        f"samples_{args.sample_mode}_train{args.train_limit}_val{args.val_limit}_"
        f"ts{args.train_stride}_vs{args.val_stride}_{cache_version}.npz"
    )
    if cache_path.exists() and not args.rebuild_cache:
        print(f"loading cached tensors from {cache_path}")
        cached = np.load(cache_path)
        return cached["train_x"], cached["train_y"], cached["val_x"], cached["val_y"]

    train_x, train_y, val_x, val_y = load_samples(args, aux_features, input_features)
    print(f"writing cached tensors to {cache_path}")
    np.savez(
        cache_path,
        train_x=train_x,
        train_y=train_y,
        val_x=val_x,
        val_y=val_y,
    )
    return train_x, train_y, val_x, val_y


def load_explicit_cache(args: argparse.Namespace, input_features: int) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    print(f"loading explicit tensor cache from {args.input_cache}")
    cached = np.load(args.input_cache)
    train_x = cached["train_x"][: args.train_limit]
    train_y = cached["train_y"][: args.train_limit]
    val_x = cached["val_x"][: args.val_limit]
    val_y = cached["val_y"][: args.val_limit]
    if args.aux_features is not None:
        aux_features = load_aux_features(args.aux_features, args.aux_exclude_columns, args.aux_include_prefixes)
        print(f"appending auxiliary features width={aux_features.width}", flush=True)
        train_x = append_aux_by_coordinate(train_x, aux_features, "train_x")
        val_x = append_aux_by_coordinate(val_x, aux_features, "val_x")
    if args.zero_aux_width:
        print(f"appending zero auxiliary features width={args.zero_aux_width}", flush=True)
        train_x = append_zero_features(train_x, args.zero_aux_width)
        val_x = append_zero_features(val_x, args.zero_aux_width)
    if train_x.shape[1] != val_x.shape[1]:
        raise RuntimeError(f"cache feature width mismatch: train={train_x.shape[1]} val={val_x.shape[1]}")
    if train_x.shape[1] != input_features:
        if args.aux_features is None and args.zero_aux_width == 0:
            print(f"explicit cache provides feature width={train_x.shape[1]} expected={input_features}", flush=True)
        else:
            raise RuntimeError(
                f"cache feature width mismatch: train={train_x.shape[1]} val={val_x.shape[1]} expected={input_features}"
            )
    return train_x, train_y, val_x, val_y


def append_zero_features(values: np.ndarray, width: int) -> np.ndarray:
    output = np.zeros((values.shape[0], values.shape[1] + width), dtype=np.float32)
    output[:, : values.shape[1]] = values
    return output


def append_aux_by_coordinate(values: np.ndarray, aux_features: "AuxFeatureSet", name: str) -> np.ndarray:
    output = np.empty((values.shape[0], values.shape[1] + aux_features.width), dtype=np.float32)
    output[:, : values.shape[1]] = values
    for index, row in enumerate(values):
        lat = float(row[0]) * 90.0
        lon = float(row[1]) * 180.0
        output[index, values.shape[1] :] = aux_features.for_lat_lon(lat, lon)
        if (index + 1) % 1_000_000 == 0 or index + 1 == values.shape[0]:
            print(f"{name} append_aux {index + 1}/{values.shape[0]}", flush=True)
    return output


def apply_aux_clip(train_x: np.ndarray, val_x: np.ndarray, clip_abs: float | None) -> None:
    if clip_abs is None or train_x.shape[1] <= BASE_INPUTS:
        return
    print(f"clipping auxiliary inputs to +/-{clip_abs}", flush=True)
    np.clip(train_x[:, BASE_INPUTS:], -clip_abs, clip_abs, out=train_x[:, BASE_INPUTS:])
    np.clip(val_x[:, BASE_INPUTS:], -clip_abs, clip_abs, out=val_x[:, BASE_INPUTS:])


def load_samples(
    args: argparse.Namespace, aux_features: "AuxFeatureSet", input_features: int
) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    if args.sample_mode == "reservoir":
        return load_samples_reservoir(args, aux_features, input_features)
    return load_samples_prefix(args, aux_features, input_features)


def load_samples_prefix(
    args: argparse.Namespace, aux_features: "AuxFeatureSet", input_features: int
) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    train_x = np.empty((args.train_limit, input_features), dtype=np.float32)
    train_y = np.empty((args.train_limit, TARGETS), dtype=np.float32)
    val_x = np.empty((args.val_limit, input_features), dtype=np.float32)
    val_y = np.empty((args.val_limit, TARGETS), dtype=np.float32)

    train_count = 0
    val_count = 0
    with gzip.open(args.data, "rt", newline="", encoding="utf-8") as handle:
        reader = csv.reader(handle)
        next(reader)
        for row_index, row in enumerate(reader, start=1):
            if train_count >= args.train_limit and val_count >= args.val_limit:
                break
            location_id = row[2]
            grid_number = parse_grid_number(location_id)
            is_validation = grid_number is not None and grid_number % 17 == 0
            if is_validation:
                if val_count >= args.val_limit or row_index % args.val_stride != 0:
                    continue
            elif train_count >= args.train_limit or row_index % args.train_stride != 0:
                continue

            features, target = parse_training_row(row, aux_features)

            if is_validation:
                val_x[val_count] = features
                val_y[val_count] = target
                val_count += 1
            else:
                train_x[train_count] = features
                train_y[train_count] = target
                train_count += 1

    if train_count == 0 or val_count == 0:
        raise RuntimeError("empty train or validation sample")
    return train_x[:train_count], train_y[:train_count], val_x[:val_count], val_y[:val_count]


def load_samples_reservoir(
    args: argparse.Namespace, aux_features: "AuxFeatureSet", input_features: int
) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    train_x = np.empty((args.train_limit, input_features), dtype=np.float32)
    train_y = np.empty((args.train_limit, TARGETS), dtype=np.float32)
    val_x = np.empty((args.val_limit, input_features), dtype=np.float32)
    val_y = np.empty((args.val_limit, TARGETS), dtype=np.float32)
    rng = np.random.default_rng(args.seed)

    train_seen = 0
    val_seen = 0
    train_count = 0
    val_count = 0
    started = time.time()
    with gzip.open(args.data, "rt", newline="", encoding="utf-8") as handle:
        reader = csv.reader(handle)
        next(reader)
        for row_index, row in enumerate(reader, start=1):
            location_id = row[2]
            grid_number = parse_grid_number(location_id)
            is_validation = grid_number is not None and grid_number % 17 == 0
            if is_validation:
                if row_index % args.val_stride != 0:
                    continue
                val_seen += 1
                if val_count < args.val_limit:
                    target_index = val_count
                    val_count += 1
                else:
                    replacement = int(rng.integers(val_seen))
                    if replacement >= args.val_limit:
                        continue
                    target_index = replacement
                features, target = parse_training_row(row, aux_features)
                val_x[target_index] = features
                val_y[target_index] = target
            else:
                if row_index % args.train_stride != 0:
                    continue
                train_seen += 1
                if train_count < args.train_limit:
                    target_index = train_count
                    train_count += 1
                else:
                    replacement = int(rng.integers(train_seen))
                    if replacement >= args.train_limit:
                        continue
                    target_index = replacement
                features, target = parse_training_row(row, aux_features)
                train_x[target_index] = features
                train_y[target_index] = target

            if row_index % 10_000_000 == 0:
                elapsed = time.time() - started
                print(
                    f"sample_scan_rows={row_index} train_seen={train_seen} val_seen={val_seen} "
                    f"train_count={train_count} val_count={val_count} elapsed={elapsed:.1f}s",
                    flush=True,
                )

    if train_count == 0 or val_count == 0:
        raise RuntimeError("empty train or validation sample")
    print(f"reservoir_seen train={train_seen} val={val_seen}")
    return train_x[:train_count], train_y[:train_count], val_x[:val_count], val_y[:val_count]


def parse_training_row(row: list[str], aux_features: "AuxFeatureSet") -> tuple[np.ndarray, np.ndarray]:
    timestamp = row[3]
    latitude = float(row[4])
    longitude = float(row[5])
    elevation = float(row[6] or 0.0)
    target = np.array([float(row[7]), float(row[8]), float(row[9]), float(row[10]), float(row[11])], dtype=np.float32)
    day_of_year, hour = parse_timestamp(timestamp)
    features = encode_features(latitude, longitude, elevation, day_of_year, hour)
    if aux_features.width:
        features = np.concatenate([features, aux_features.for_location(row[2])]).astype(np.float32)
    return features, target


class AuxFeatureSet:
    def __init__(
        self,
        columns: list[str],
        by_location: dict[str, np.ndarray],
        by_coord: dict[tuple[int, int], np.ndarray],
        cache_key: str,
    ) -> None:
        self.columns = columns
        self.by_location = by_location
        self.by_coord = by_coord
        self.cache_key = cache_key
        self.width = len(columns)
        self._empty = np.empty((0,), dtype=np.float32)

    def for_location(self, location_id: str) -> np.ndarray:
        if self.width == 0:
            return self._empty
        try:
            return self.by_location[location_id]
        except KeyError as exc:
            raise KeyError(f"aux features missing location_id={location_id}") from exc

    def for_lat_lon(self, lat: float, lon: float) -> np.ndarray:
        if self.width == 0:
            return self._empty
        key = coord_key(lat, lon)
        try:
            return self.by_coord[key]
        except KeyError as exc:
            raise KeyError(f"aux features missing lat={lat:.6f} lon={lon:.6f} key={key}") from exc


def load_aux_features(path: Path | None, exclude_columns: str, include_prefixes: str | None) -> AuxFeatureSet:
    if path is None:
        return AuxFeatureSet([], {}, {}, "none")

    excluded = {column.strip() for column in exclude_columns.split(",") if column.strip()}
    prefixes = tuple(prefix.strip() for prefix in include_prefixes.split(",") if prefix.strip()) if include_prefixes else ()
    with path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        if reader.fieldnames is None or "location_id" not in reader.fieldnames:
            raise ValueError("aux feature CSV must contain a location_id column")
        columns = [column for column in reader.fieldnames if column not in excluded]
        if prefixes:
            columns = [column for column in columns if column.startswith(prefixes)]
        if not columns:
            raise ValueError("aux feature CSV has no usable columns after exclusions")
        location_ids: list[str] = []
        coords: list[tuple[int, int]] = []
        matrix_rows: list[list[float]] = []
        for row in reader:
            location_ids.append(row["location_id"])
            coords.append(coord_key(float(row["latitude"]), float(row["longitude"])))
            matrix_rows.append([float(row[column]) for column in columns])

    matrix = np.array(matrix_rows, dtype=np.float32)
    mean = matrix.mean(axis=0).astype(np.float32)
    std = matrix.std(axis=0).astype(np.float32)
    std = np.maximum(std, 1e-6).astype(np.float32)
    normalized = (matrix - mean) / std
    by_location = {location_id: normalized[index] for index, location_id in enumerate(location_ids)}
    by_coord = {coord: normalized[index] for index, coord in enumerate(coords)}
    digest = hashlib.sha256()
    digest.update(path.read_bytes())
    digest.update(",".join(columns).encode("utf-8"))
    cache_key = digest.hexdigest()[:12]
    print(f"loaded aux_features path={path} rows={len(location_ids)} columns={len(columns)} cache_key={cache_key}")
    return AuxFeatureSet(columns, by_location, by_coord, cache_key)


def coord_key(lat: float, lon: float) -> tuple[int, int]:
    return int(round(lat * 1000.0)), int(round(lon * 1000.0))


def parse_grid_number(location_id: str) -> int | None:
    if not location_id.startswith("grid_"):
        return None
    return int(location_id.removeprefix("grid_"))


def parse_timestamp(timestamp: str) -> tuple[int, int]:
    year = int(timestamp[0:4])
    month = int(timestamp[5:7])
    day = int(timestamp[8:10])
    hour = int(timestamp[11:13])
    days_before_month_common = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334]
    day_of_year = days_before_month_common[month - 1] + day
    if month > 2 and is_leap_year(year):
        day_of_year += 1
    return day_of_year, hour


def is_leap_year(year: int) -> bool:
    return (year % 4 == 0 and year % 100 != 0) or year % 400 == 0


def encode_features(latitude: float, longitude: float, elevation: float, day_of_year: int, hour: int) -> np.ndarray:
    features: list[float] = []
    lat_norm = latitude / 90.0
    lon_norm = longitude / 180.0
    elev_norm = max(-1.0, min(3.0, elevation / 3000.0))
    day_angle = 2.0 * math.pi * (day_of_year - 1.0) / 366.0
    hour_angle = 2.0 * math.pi * hour / 24.0
    base_day_sin = math.sin(day_angle)
    base_day_cos = math.cos(day_angle)
    base_hour_sin = math.sin(hour_angle)
    base_hour_cos = math.cos(hour_angle)

    features.extend([lat_norm, lon_norm, elev_norm, base_day_sin, base_day_cos, base_hour_sin, base_hour_cos])
    for harmonic in range(2, 9):
        angle = day_angle * harmonic
        features.extend([math.sin(angle), math.cos(angle)])
    for harmonic in range(2, 7):
        angle = hour_angle * harmonic
        features.extend([math.sin(angle), math.cos(angle)])
    for harmonic in range(1, 7):
        angle = math.pi * lat_norm * harmonic
        features.extend([math.sin(angle), math.cos(angle)])
    for harmonic in range(1, 7):
        angle = math.pi * lon_norm * harmonic
        features.extend([math.sin(angle), math.cos(angle)])
    features.extend(
        [
            lat_norm * base_day_sin,
            lat_norm * base_day_cos,
            lon_norm * base_day_sin,
            lon_norm * base_day_cos,
            base_day_sin * base_hour_sin,
            base_day_sin * base_hour_cos,
            base_day_cos * base_hour_sin,
            base_day_cos * base_hour_cos,
            elev_norm * base_day_sin,
        ]
    )
    if len(features) != BASE_INPUTS:
        raise RuntimeError(f"expected {BASE_INPUTS} features, got {len(features)}")
    return np.array(features, dtype=np.float32)


def evaluate(
    model: WeatherMlp,
    val_x: torch.Tensor,
    val_y: np.ndarray,
    target_mean: np.ndarray,
    target_std: np.ndarray,
    quantiles: torch.Tensor,
    device: torch.device,
) -> dict[str, object]:
    model.eval()
    predictions: list[np.ndarray] = []
    with torch.no_grad():
        for start in range(0, len(val_x), 65536):
            batch = val_x[start : start + 65536]
            out = model(batch).view(-1, TARGETS, 3).detach().cpu().numpy()
            predictions.append(out)
    pred = np.concatenate(predictions, axis=0)
    pred = pred * target_std.reshape(1, TARGETS, 1) + target_mean.reshape(1, TARGETS, 1)

    crossing = np.mean((pred[:, :, 0] > pred[:, :, 1]) | (pred[:, :, 1] > pred[:, :, 2]))
    sorted_pred = np.sort(pred, axis=2)
    q10 = sorted_pred[:, :, 0]
    q50 = sorted_pred[:, :, 1]
    q90 = sorted_pred[:, :, 2]
    errors = val_y - q50
    mae = np.mean(np.abs(errors), axis=0)
    rmse = np.sqrt(np.mean(errors * errors, axis=0))
    coverage = np.mean((val_y >= q10) & (val_y <= q90), axis=0)

    pinball = 0.0
    for index, quantile in enumerate([0.1, 0.5, 0.9]):
        q_error = val_y - sorted_pred[:, :, index]
        pinball += np.maximum(quantile * q_error, (quantile - 1.0) * q_error).mean()
    pinball /= 3.0

    return {
        "pinball_loss": float(pinball),
        "mae": [float(value) for value in mae],
        "rmse": [float(value) for value in rmse],
        "coverage_p10_p90": [float(value) for value in coverage],
        "crossing_rate": float(crossing),
    }


if __name__ == "__main__":
    raise SystemExit(main())
