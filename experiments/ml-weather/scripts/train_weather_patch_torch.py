#!/usr/bin/env python3
"""Train a weather model with static elevation and terrain grid-map patches."""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

import numpy as np
import torch
from torch import nn

from train_weather_mlp_torch import BASE_INPUTS, QUANTILES, TARGETS, TARGET_NAMES, ResidualBlock, pinball_loss


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input-cache", type=Path, required=True, help="base 64-feature train/validation .npz cache")
    parser.add_argument(
        "--patches",
        type=Path,
        nargs="+",
        required=True,
        help="one or more location patch .npz files from collect_aux_geography.py patches",
    )
    parser.add_argument("--out-dir", type=Path, default=Path("results/weather_patch_residual"))
    parser.add_argument("--train-limit", type=int, default=1_000_000)
    parser.add_argument("--val-limit", type=int, default=100_000)
    parser.add_argument("--residual-width", type=int, default=384)
    parser.add_argument("--residual-blocks", type=int, default=6)
    parser.add_argument("--residual-scale", type=float, default=0.5)
    parser.add_argument("--geo-width", type=int, default=128)
    parser.add_argument("--scale-width", type=int, default=64)
    parser.add_argument("--terrain-embedding-width", type=int, default=6)
    parser.add_argument("--geo-scale-init", type=float, default=0.1)
    parser.add_argument("--epochs", type=int, default=20)
    parser.add_argument("--batch-size", type=int, default=32768)
    parser.add_argument("--learning-rate", type=float, default=1e-3)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--sort-by-location", action=argparse.BooleanOptionalAction, default=True)
    parser.add_argument("--resident-device", action=argparse.BooleanOptionalAction, default=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    validate_args(args)
    args.out_dir.mkdir(parents=True, exist_ok=True)
    torch.manual_seed(args.seed)
    np.random.seed(args.seed)
    torch.backends.cuda.matmul.allow_tf32 = True
    torch.backends.cudnn.allow_tf32 = True

    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    print(f"device={device}")
    if device.type == "cuda":
        print(f"gpu={torch.cuda.get_device_name(0)}")

    started = time.time()
    train_x, train_y, val_x, val_y = load_cache(args.input_cache, args.train_limit, args.val_limit)
    patch_tables, patch_meta = load_patch_tables(args.patches)
    train_patch_idx = build_patch_indices(train_x, patch_meta, "train")
    val_patch_idx = build_patch_indices(val_x, patch_meta, "val")
    print(
        f"loaded train_rows={len(train_x)} val_rows={len(val_x)} patch_scales={patch_tables.elevation.shape[0]} "
        f"locations={patch_tables.elevation.shape[1]} patch_shape={tuple(patch_tables.elevation.shape[2:])} "
        f"in {time.time() - started:.1f}s",
        flush=True,
    )

    if args.sort_by_location:
        train_x, train_y, train_patch_idx = sort_by_patch(train_x, train_y, train_patch_idx, "train")
        val_x, val_y, val_patch_idx = sort_by_patch(val_x, val_y, val_patch_idx, "val")

    target_mean = train_y.mean(axis=0).astype(np.float32)
    target_std = np.maximum(train_y.std(axis=0).astype(np.float32), 1.0).astype(np.float32)
    train_y_norm = ((train_y - target_mean) / target_std).astype(np.float32)

    model = WeatherPatchResidualMlp(
        scale_count=patch_tables.elevation.shape[0],
        scale_width=args.scale_width,
        geo_width=args.geo_width,
        terrain_embedding_width=args.terrain_embedding_width,
        residual_width=args.residual_width,
        residual_blocks=args.residual_blocks,
        residual_scale=args.residual_scale,
        geo_scale_init=args.geo_scale_init,
    ).to(device)
    parameters = sum(parameter.numel() for parameter in model.parameters())
    print(f"parameters={parameters}")

    elevation_t = torch.from_numpy(patch_tables.elevation).to(device)
    terrain_t = torch.from_numpy(patch_tables.terrain).to(device)
    if args.resident_device:
        print("moving sample tensors to device", flush=True)
        train_x_t = torch.from_numpy(train_x).to(device)
        train_y_t = torch.from_numpy(train_y_norm).to(device)
        train_patch_idx_t = torch.from_numpy(train_patch_idx).to(device)
        val_x_t = torch.from_numpy(val_x).to(device)
        val_patch_idx_t = torch.from_numpy(val_patch_idx).to(device)
    else:
        train_x_t = torch.from_numpy(train_x)
        train_y_t = torch.from_numpy(train_y_norm)
        train_patch_idx_t = torch.from_numpy(train_patch_idx)
        val_x_t = torch.from_numpy(val_x)
        val_patch_idx_t = torch.from_numpy(val_patch_idx)

    optimizer = torch.optim.AdamW(model.parameters(), lr=args.learning_rate)
    quantiles = QUANTILES.to(device)
    rng = np.random.default_rng(args.seed)
    history: list[dict[str, object]] = []
    best_pinball = float("inf")
    best_epoch = 0

    for epoch in range(1, args.epochs + 1):
        model.train()
        total_loss = 0.0
        batches = 0
        starts = np.arange(0, len(train_x), args.batch_size, dtype=np.int64)
        rng.shuffle(starts)
        epoch_started = time.time()
        for batch_number, start in enumerate(starts, start=1):
            stop = min(int(start) + args.batch_size, len(train_x))
            if args.resident_device:
                batch_x = train_x_t[start:stop]
                batch_y = train_y_t[start:stop]
                batch_patch_idx = train_patch_idx_t[start:stop]
            else:
                batch_x = train_x_t[start:stop].to(device, non_blocking=True)
                batch_y = train_y_t[start:stop].to(device, non_blocking=True)
                batch_patch_idx = train_patch_idx_t[start:stop].to(device, non_blocking=True)
            optimizer.zero_grad(set_to_none=True)
            predictions = model(batch_x, batch_patch_idx, elevation_t, terrain_t).view(-1, TARGETS, 3)
            loss = pinball_loss(predictions, batch_y, quantiles)
            loss.backward()
            optimizer.step()
            total_loss += float(loss.detach().cpu())
            batches += 1
            if batch_number == 1 or batch_number % 250 == 0 or batch_number == len(starts):
                elapsed = time.time() - epoch_started
                rate = batch_number / max(elapsed, 0.001)
                remaining = (len(starts) - batch_number) / max(rate, 0.001)
                print(
                    f"epoch={epoch} batch={batch_number}/{len(starts)} "
                    f"rate={rate:.2f}/s eta={remaining/60:.1f}m",
                    flush=True,
                )

        metrics = evaluate_patch(model, val_x_t, val_patch_idx_t, elevation_t, terrain_t, val_y, target_mean, target_std, quantiles, device)
        metrics["epoch"] = epoch
        metrics["train_pinball"] = total_loss / max(batches, 1)
        history.append(metrics)
        if metrics["pinball_loss"] < best_pinball:
            best_pinball = float(metrics["pinball_loss"])
            best_epoch = epoch
            torch.save(
                {
                    "model_state_dict": model.state_dict(),
                    "epoch": epoch,
                    "pinball_loss": best_pinball,
                    "target_mean": target_mean,
                    "target_std": target_std,
                    "target_names": TARGET_NAMES,
                    "patch_radii_km": patch_meta["radii_km"],
                    "patch_shape": list(patch_tables.elevation.shape[2:]),
                    "geo_width": args.geo_width,
                    "scale_width": args.scale_width,
                    "terrain_embedding_width": args.terrain_embedding_width,
                    "residual_width": args.residual_width,
                    "residual_blocks": args.residual_blocks,
                    "residual_scale": args.residual_scale,
                    "base_input_features": BASE_INPUTS,
                },
                args.out_dir / "best_model.pt",
            )
        print(
            f"epoch={epoch} train_pinball={metrics['train_pinball']:.6f} "
            f"val_pinball={metrics['pinball_loss']:.6f} val_mae={metrics['mae']} "
            f"geo_scale={model.geo_scale_value():.6f} best_epoch={best_epoch} best_pinball={best_pinball:.6f}",
            flush=True,
        )

    final_metrics = history[-1]
    output = {
        "model": "weather_patch_torch",
        "architecture": "residual-mlp-plus-multiscale-geography-cnn",
        "input_cache": str(args.input_cache),
        "patches": [str(path) for path in args.patches],
        "patch_radii_km": patch_meta["radii_km"],
        "base_input_features": BASE_INPUTS,
        "patch_scales": patch_tables.elevation.shape[0],
        "patch_shape": list(patch_tables.elevation.shape[2:]),
        "geo_width": args.geo_width,
        "scale_width": args.scale_width,
        "terrain_embedding_width": args.terrain_embedding_width,
        "geo_scale_init": args.geo_scale_init,
        "geo_scale_final": model.geo_scale_value(),
        "best_epoch": best_epoch,
        "best_pinball_loss": best_pinball,
        "residual_width": args.residual_width,
        "residual_blocks": args.residual_blocks,
        "residual_scale": args.residual_scale,
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
        "resident_device": args.resident_device,
        "sort_by_location": args.sort_by_location,
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
            "patch_radii_km": patch_meta["radii_km"],
            "patch_shape": list(patch_tables.elevation.shape[2:]),
            "geo_width": args.geo_width,
            "scale_width": args.scale_width,
            "terrain_embedding_width": args.terrain_embedding_width,
            "residual_width": args.residual_width,
            "residual_blocks": args.residual_blocks,
            "residual_scale": args.residual_scale,
            "base_input_features": BASE_INPUTS,
        },
        args.out_dir / "model.pt",
    )
    print(f"wrote {metrics_path}")
    print(f"wrote {args.out_dir / 'model.pt'}")
    print(f"wrote {args.out_dir / 'best_model.pt'}")
    return 0


def validate_args(args: argparse.Namespace) -> None:
    if args.train_limit <= 0 or args.val_limit <= 0:
        raise ValueError("train and validation limits must be positive")
    if args.batch_size <= 0:
        raise ValueError("--batch-size must be positive")
    if args.residual_width <= 0 or args.residual_blocks <= 0 or args.geo_width <= 0 or args.scale_width <= 0:
        raise ValueError("model widths and block count must be positive")
    if args.terrain_embedding_width <= 0:
        raise ValueError("--terrain-embedding-width must be positive")


def load_cache(path: Path, train_limit: int, val_limit: int) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    print(f"loading cache {path}", flush=True)
    cached = np.load(path)
    train_x = np.asarray(cached["train_x"][:train_limit], dtype=np.float32)
    train_y = np.asarray(cached["train_y"][:train_limit], dtype=np.float32)
    val_x = np.asarray(cached["val_x"][:val_limit], dtype=np.float32)
    val_y = np.asarray(cached["val_y"][:val_limit], dtype=np.float32)
    if train_x.shape[1] != BASE_INPUTS or val_x.shape[1] != BASE_INPUTS:
        raise RuntimeError(f"expected base cache width {BASE_INPUTS}, got train={train_x.shape[1]} val={val_x.shape[1]}")
    return train_x, train_y, val_x, val_y


class PatchTables:
    def __init__(self, elevation: np.ndarray, terrain: np.ndarray) -> None:
        self.elevation = elevation
        self.terrain = terrain


def load_patch_tables(paths: list[Path]) -> tuple[PatchTables, dict[str, object]]:
    elevations: list[np.ndarray] = []
    terrains: list[np.ndarray] = []
    coord_keys_reference: list[int] | None = None
    coord_to_index: dict[int, int] | None = None
    radii_km: list[float] = []
    patch_pixels: int | None = None

    for path in paths:
        print(f"loading patches {path}", flush=True)
        data = np.load(path)
        elevation = np.asarray(data["elevation_m"], dtype=np.float32)
        terrain = np.asarray(data["terrain_class"], dtype=np.uint8)
        elevation = np.clip(np.nan_to_num(elevation / 3000.0, nan=0.0), -2.0, 3.0).astype(np.float32)
        latitude = np.asarray(data["latitude"], dtype=np.float32)
        longitude = np.asarray(data["longitude"], dtype=np.float32)
        coord_keys = [pack_coord_key(float(lat), float(lon)) for lat, lon in zip(latitude, longitude)]
        if coord_keys_reference is None:
            coord_keys_reference = coord_keys
            coord_to_index = {key: index for index, key in enumerate(coord_keys)}
            patch_pixels = int(elevation.shape[-1])
        elif coord_keys != coord_keys_reference:
            raise RuntimeError(f"patch file has different location order: {path}")
        if elevation.shape != terrain.shape:
            raise RuntimeError(f"elevation and terrain shapes differ in {path}: {elevation.shape} vs {terrain.shape}")
        if patch_pixels is not None and elevation.shape[-1] != patch_pixels:
            raise RuntimeError(f"all patch files must use the same pixel size; {path} has {elevation.shape[-1]}")
        elevations.append(elevation)
        terrains.append(terrain)
        radii_km.append(float(np.asarray(data["radius_km"]).item()) if "radius_km" in data.files else float("nan"))

    if coord_to_index is None:
        raise RuntimeError("no patches loaded")
    return PatchTables(np.stack(elevations, axis=0), np.stack(terrains, axis=0)), {
        "coord_to_index": coord_to_index,
        "radii_km": radii_km,
        "patch_pixels": patch_pixels,
    }


def build_patch_indices(values: np.ndarray, patch_meta: dict[str, object], name: str) -> np.ndarray:
    coord_to_index = patch_meta["coord_to_index"]
    if not isinstance(coord_to_index, dict):
        raise RuntimeError("invalid patch metadata")
    lat_keys = np.rint(values[:, 0].astype(np.float64) * 90_000.0).astype(np.int64)
    lon_keys = np.rint(values[:, 1].astype(np.float64) * 180_000.0).astype(np.int64)
    packed = pack_coord_key_arrays(lat_keys, lon_keys)
    unique_keys, inverse = np.unique(packed, return_inverse=True)
    mapped_unique = np.empty((len(unique_keys),), dtype=np.int64)
    missing: list[int] = []
    for index, key in enumerate(unique_keys):
        patch_index = coord_to_index.get(int(key))
        if patch_index is None:
            missing.append(int(key))
            mapped_unique[index] = -1
        else:
            mapped_unique[index] = patch_index
    if missing:
        raise KeyError(f"{name} has {len(missing)} coordinates missing from patch file; first={missing[0]}")
    result = mapped_unique[inverse]
    print(f"{name} unique_patch_locations={len(unique_keys)}", flush=True)
    return result.astype(np.int64)


def sort_by_patch(
    values: np.ndarray, targets: np.ndarray, patch_idx: np.ndarray, name: str
) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    started = time.time()
    order = np.argsort(patch_idx, kind="stable")
    print(f"sorting {name} rows by patch index", flush=True)
    sorted_values = np.ascontiguousarray(values[order])
    sorted_targets = np.ascontiguousarray(targets[order])
    sorted_patch_idx = np.ascontiguousarray(patch_idx[order])
    print(f"sorted {name} rows in {time.time() - started:.1f}s", flush=True)
    return sorted_values, sorted_targets, sorted_patch_idx


def pack_coord_key(lat: float, lon: float) -> int:
    lat_key = int(round(lat * 1000.0))
    lon_key = int(round(lon * 1000.0))
    return ((lat_key + 90_000) << 32) | (lon_key + 180_000)


def pack_coord_key_arrays(lat_keys: np.ndarray, lon_keys: np.ndarray) -> np.ndarray:
    return ((lat_keys + 90_000) << 32) | (lon_keys + 180_000)


class PatchScaleEncoder(nn.Module):
    def __init__(self, terrain_embedding_width: int, scale_width: int) -> None:
        super().__init__()
        input_channels = 1 + terrain_embedding_width
        self.terrain_embedding = nn.Embedding(256, terrain_embedding_width)
        self.net = nn.Sequential(
            nn.Conv2d(input_channels, 32, kernel_size=3, padding=1),
            nn.SiLU(),
            nn.Conv2d(32, 64, kernel_size=3, stride=2, padding=1),
            nn.SiLU(),
            nn.Conv2d(64, 96, kernel_size=3, stride=2, padding=1),
            nn.SiLU(),
            nn.Conv2d(96, 96, kernel_size=3, stride=2, padding=1),
            nn.SiLU(),
            nn.AdaptiveAvgPool2d(1),
            nn.Flatten(),
            nn.Linear(96, scale_width),
            nn.SiLU(),
        )

    def forward(self, elevation: torch.Tensor, terrain: torch.Tensor) -> torch.Tensor:
        terrain_channels = self.terrain_embedding(terrain.long()).permute(0, 3, 1, 2)
        values = torch.cat([elevation.unsqueeze(1), terrain_channels], dim=1)
        return self.net(values)


class WeatherPatchResidualMlp(nn.Module):
    def __init__(
        self,
        scale_count: int,
        scale_width: int,
        geo_width: int,
        terrain_embedding_width: int,
        residual_width: int,
        residual_blocks: int,
        residual_scale: float,
        geo_scale_init: float,
    ) -> None:
        super().__init__()
        self.base_input = nn.Linear(BASE_INPUTS, residual_width)
        self.scale_encoders = nn.ModuleList(
            PatchScaleEncoder(terrain_embedding_width, scale_width) for _ in range(scale_count)
        )
        self.geo_project = nn.Sequential(
            nn.Linear(scale_count * scale_width, geo_width),
            nn.SiLU(),
            nn.Linear(geo_width, residual_width),
        )
        self.geo_scale = nn.Parameter(torch.tensor(float(geo_scale_init), dtype=torch.float32))
        self.blocks = nn.Sequential(*(ResidualBlock(residual_width, residual_scale) for _ in range(residual_blocks)))
        self.output = nn.Sequential(
            nn.LayerNorm(residual_width),
            nn.SiLU(),
            nn.Linear(residual_width, TARGETS * 3),
        )

    def forward(
        self,
        base_values: torch.Tensor,
        patch_indices: torch.Tensor,
        elevation_table: torch.Tensor,
        terrain_table: torch.Tensor,
    ) -> torch.Tensor:
        unique_indices, inverse = torch.unique(patch_indices, sorted=False, return_inverse=True)
        scale_embeddings: list[torch.Tensor] = []
        for scale_index, encoder in enumerate(self.scale_encoders):
            unique_elevation = elevation_table[scale_index].index_select(0, unique_indices)
            unique_terrain = terrain_table[scale_index].index_select(0, unique_indices)
            scale_embeddings.append(encoder(unique_elevation, unique_terrain))
        geo_unique = self.geo_project(torch.cat(scale_embeddings, dim=1))
        geo = geo_unique.index_select(0, inverse)
        hidden = self.base_input(base_values) + self.geo_scale * geo
        hidden = self.blocks(hidden)
        return self.output(hidden)

    def geo_scale_value(self) -> float:
        return float(self.geo_scale.detach().cpu())


def evaluate_patch(
    model: WeatherPatchResidualMlp,
    val_x: torch.Tensor,
    val_patch_idx: torch.Tensor,
    elevation_table: torch.Tensor,
    terrain_table: torch.Tensor,
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
            stop = min(start + 65536, len(val_x))
            batch_x = val_x[start:stop]
            batch_patch_idx = val_patch_idx[start:stop]
            if batch_x.device != device:
                batch_x = batch_x.to(device, non_blocking=True)
                batch_patch_idx = batch_patch_idx.to(device, non_blocking=True)
            out = model(batch_x, batch_patch_idx, elevation_table, terrain_table).view(-1, TARGETS, 3).detach().cpu().numpy()
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
