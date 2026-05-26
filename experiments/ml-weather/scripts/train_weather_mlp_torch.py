#!/usr/bin/env python3
"""Train weather quantile MLPs with PyTorch/CUDA for the ML weather experiment."""

from __future__ import annotations

import argparse
import csv
import gzip
import json
import math
import time
from pathlib import Path

import numpy as np
import torch
from torch import nn
from torch.utils.data import DataLoader, TensorDataset

INPUTS = 64
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
    parser.add_argument("--epochs", type=int, default=20)
    parser.add_argument("--batch-size", type=int, default=8192)
    parser.add_argument("--learning-rate", type=float, default=1e-3)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--cache-dir", type=Path, default=None)
    parser.add_argument("--rebuild-cache", action="store_true")
    parser.add_argument("--resident-device", action="store_true")
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
    torch.manual_seed(args.seed)
    np.random.seed(args.seed)

    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    print(f"device={device}")
    if device.type == "cuda":
        print(f"gpu={torch.cuda.get_device_name(0)}")

    started = time.time()
    train_x, train_y, val_x, val_y = load_or_build_samples(args)
    print(f"loaded train_rows={len(train_x)} val_rows={len(val_x)} in {time.time() - started:.1f}s")

    target_mean = train_y.mean(axis=0).astype(np.float32)
    target_std = train_y.std(axis=0).astype(np.float32)
    target_std = np.maximum(target_std, 1.0).astype(np.float32)
    train_y_norm = (train_y - target_mean) / target_std

    model = WeatherMlp(hidden_layers).to(device)
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
        "input_features": INPUTS,
        "hidden_width": args.hidden_width,
        "hidden_layers": hidden_layers,
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
            "hidden_layers": hidden_layers,
            "input_features": INPUTS,
        },
        args.out_dir / "model.pt",
    )
    print(f"wrote {metrics_path}")
    print(f"wrote {args.out_dir / 'model.pt'}")
    return 0


class WeatherMlp(nn.Module):
    def __init__(self, hidden_layers: list[int]) -> None:
        super().__init__()
        layers: list[nn.Module] = []
        previous_width = INPUTS
        for hidden_width in hidden_layers:
            layers.append(nn.Linear(previous_width, hidden_width))
            layers.append(nn.SiLU())
            previous_width = hidden_width
        layers.append(nn.Linear(previous_width, TARGETS * 3))
        self.net = nn.Sequential(*layers)

    def forward(self, values: torch.Tensor) -> torch.Tensor:
        return self.net(values)


def pinball_loss(predictions: torch.Tensor, targets: torch.Tensor, quantiles: torch.Tensor) -> torch.Tensor:
    errors = targets.unsqueeze(-1) - predictions
    losses = torch.maximum(quantiles * errors, (quantiles - 1.0) * errors)
    return losses.mean()


def load_or_build_samples(args: argparse.Namespace) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    if args.cache_dir is None:
        return load_samples(args)

    args.cache_dir.mkdir(parents=True, exist_ok=True)
    cache_path = args.cache_dir / (
        f"samples_train{args.train_limit}_val{args.val_limit}_"
        f"ts{args.train_stride}_vs{args.val_stride}_v1.npz"
    )
    if cache_path.exists() and not args.rebuild_cache:
        print(f"loading cached tensors from {cache_path}")
        cached = np.load(cache_path)
        return cached["train_x"], cached["train_y"], cached["val_x"], cached["val_y"]

    train_x, train_y, val_x, val_y = load_samples(args)
    print(f"writing cached tensors to {cache_path}")
    np.savez(
        cache_path,
        train_x=train_x,
        train_y=train_y,
        val_x=val_x,
        val_y=val_y,
    )
    return train_x, train_y, val_x, val_y


def load_samples(args: argparse.Namespace) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    train_x = np.empty((args.train_limit, INPUTS), dtype=np.float32)
    train_y = np.empty((args.train_limit, TARGETS), dtype=np.float32)
    val_x = np.empty((args.val_limit, INPUTS), dtype=np.float32)
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

            timestamp = row[3]
            latitude = float(row[4])
            longitude = float(row[5])
            elevation = float(row[6] or 0.0)
            target = np.array([float(row[7]), float(row[8]), float(row[9]), float(row[10]), float(row[11])], dtype=np.float32)
            day_of_year, hour = parse_timestamp(timestamp)
            features = encode_features(latitude, longitude, elevation, day_of_year, hour)

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
    if len(features) != INPUTS:
        raise RuntimeError(f"expected {INPUTS} features, got {len(features)}")
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
