#!/usr/bin/env python3
"""Train a compact neural approximation of location/month/hour climate normals."""

from __future__ import annotations

import argparse
import json
import math
import time
from pathlib import Path

import numpy as np
import torch
from torch import nn

TARGETS = 10
TARGET_NAMES = [
    "ghi_mean_w_m2",
    "dni_mean_w_m2",
    "dhi_mean_w_m2",
    "temp_mean_c",
    "wind_mean_m_s",
    "ghi_std_w_m2",
    "dni_std_w_m2",
    "dhi_std_w_m2",
    "temp_std_c",
    "wind_std_m_s",
]
INPUT_FEATURES = 66
MONTH_HOURS = 12 * 24


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--normals-cache", type=Path, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("--hidden-width", type=int, default=384)
    parser.add_argument("--residual-blocks", type=int, default=6)
    parser.add_argument("--residual-scale", type=float, default=0.5)
    parser.add_argument("--epochs", type=int, default=80)
    parser.add_argument("--batch-size", type=int, default=65536)
    parser.add_argument("--learning-rate", type=float, default=1e-3)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--val-location-stride", type=int, default=17)
    parser.add_argument("--fit-all", action="store_true", help="train on all locations and evaluate on the same full table")
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
    print(f"device={device}", flush=True)
    if device.type == "cuda":
        print(f"gpu={torch.cuda.get_device_name(0)}", flush=True)

    started = time.time()
    features, targets, row_meta = load_samples(args.normals_cache)
    train_indices, val_indices = split_indices(row_meta["location_index"], args.val_location_stride, args.fit_all)
    train_x = features[train_indices]
    train_y = targets[train_indices]
    val_x = features[val_indices]
    val_y = targets[val_indices]
    target_mean = train_y.mean(axis=0).astype(np.float32)
    target_std = np.maximum(train_y.std(axis=0).astype(np.float32), 1e-3).astype(np.float32)
    train_y_norm = ((train_y - target_mean) / target_std).astype(np.float32)
    print(
        f"loaded rows={len(features)} train={len(train_x)} val={len(val_x)} "
        f"locations={row_meta['location_count']} in {time.time() - started:.1f}s",
        flush=True,
    )

    model = ClimateNormalMlp(INPUT_FEATURES, args.hidden_width, args.residual_blocks, args.residual_scale).to(device)
    parameters = sum(parameter.numel() for parameter in model.parameters())
    print(f"parameters={parameters}", flush=True)

    if args.resident_device:
        train_x_t = torch.from_numpy(train_x).to(device)
        train_y_t = torch.from_numpy(train_y_norm).to(device)
        val_x_t = torch.from_numpy(val_x).to(device)
    else:
        train_x_t = torch.from_numpy(train_x)
        train_y_t = torch.from_numpy(train_y_norm)
        val_x_t = torch.from_numpy(val_x)
    optimizer = torch.optim.AdamW(model.parameters(), lr=args.learning_rate)
    rng = np.random.default_rng(args.seed)
    history: list[dict[str, object]] = []
    best_mae_mean = float("inf")
    best_epoch = 0

    for epoch in range(1, args.epochs + 1):
        model.train()
        total_loss = 0.0
        batches = 0
        starts = np.arange(0, len(train_x), args.batch_size, dtype=np.int64)
        rng.shuffle(starts)
        for start in starts:
            stop = min(int(start) + args.batch_size, len(train_x))
            if args.resident_device:
                batch_x = train_x_t[start:stop]
                batch_y = train_y_t[start:stop]
            else:
                batch_x = train_x_t[start:stop].to(device, non_blocking=True)
                batch_y = train_y_t[start:stop].to(device, non_blocking=True)
            optimizer.zero_grad(set_to_none=True)
            pred = model(batch_x)
            loss = torch.mean((pred - batch_y) ** 2)
            loss.backward()
            optimizer.step()
            total_loss += float(loss.detach().cpu())
            batches += 1

        metrics = evaluate(model, val_x_t, val_y, target_mean, target_std, device)
        metrics["epoch"] = epoch
        metrics["train_mse_norm"] = total_loss / max(batches, 1)
        history.append(metrics)
        if metrics["mae_mean"] < best_mae_mean:
            best_mae_mean = float(metrics["mae_mean"])
            best_epoch = epoch
            save_model(args.out_dir / "best_model.pt", model, args, target_mean, target_std, row_meta, parameters, best_epoch, metrics)
        print(
            f"epoch={epoch} train_mse_norm={metrics['train_mse_norm']:.6f} "
            f"val_mae_mean={metrics['mae_mean']:.6f} val_mae={metrics['mae']} "
            f"best_epoch={best_epoch} best_mae_mean={best_mae_mean:.6f}",
            flush=True,
        )

    final_metrics = history[-1]
    output = {
        "model": "climate_normals_torch",
        "input_features": INPUT_FEATURES,
        "outputs": TARGETS,
        "target_names": TARGET_NAMES,
        "normals_cache": str(args.normals_cache),
        "hidden_width": args.hidden_width,
        "residual_blocks": args.residual_blocks,
        "residual_scale": args.residual_scale,
        "parameters": parameters,
        "model_size_bytes_estimate_float32": parameters * 4,
        "epochs": args.epochs,
        "batch_size": args.batch_size,
        "learning_rate": args.learning_rate,
        "val_location_stride": args.val_location_stride,
        "fit_all": args.fit_all,
        "target_mean": target_mean.tolist(),
        "target_std": target_std.tolist(),
        "device": str(device),
        "gpu": torch.cuda.get_device_name(0) if device.type == "cuda" else None,
        "best_epoch": best_epoch,
        "best_mae_mean": best_mae_mean,
        "row_meta": serializable_meta(row_meta),
        "history": history,
        **{key: value for key, value in final_metrics.items() if key != "epoch"},
    }
    metrics_path = args.out_dir / "metrics.json"
    metrics_path.write_text(json.dumps(output, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    save_model(args.out_dir / "model.pt", model, args, target_mean, target_std, row_meta, parameters, args.epochs, final_metrics)
    print(f"wrote {metrics_path}", flush=True)
    print(f"wrote {args.out_dir / 'model.pt'}", flush=True)
    print(f"wrote {args.out_dir / 'best_model.pt'}", flush=True)
    return 0


def validate_args(args: argparse.Namespace) -> None:
    if args.hidden_width <= 0 or args.residual_blocks <= 0 or args.batch_size <= 0:
        raise ValueError("model widths, blocks, and batch size must be positive")
    if args.val_location_stride <= 1 and not args.fit_all:
        raise ValueError("--val-location-stride must be > 1 unless --fit-all is set")


def load_samples(path: Path) -> tuple[np.ndarray, np.ndarray, dict[str, object]]:
    if path.is_dir():
        means = np.asarray(np.load(path / "climate_normals.npy"), dtype=np.float32)
        stds = np.asarray(np.load(path / "climate_normal_std.npy"), dtype=np.float32)
        counts = np.asarray(np.load(path / "climate_normal_counts.npy"), dtype=np.int32)
        location_keys = np.asarray(np.load(path / "location_keys.npy"), dtype=np.int64)
    else:
        data = np.load(path)
        means = np.asarray(data["climate_normals"], dtype=np.float32)
        stds = np.asarray(data["climate_normal_std"], dtype=np.float32)
        counts = np.asarray(data["climate_normal_counts"], dtype=np.int32)
        location_keys = np.asarray(data["location_keys"], dtype=np.int64)
    if means.shape != stds.shape or means.shape[:2] != counts.shape:
        raise RuntimeError(f"normal shape mismatch: mean={means.shape} std={stds.shape} count={counts.shape}")
    location_count = means.shape[0]
    month_hours = means.shape[1]
    if month_hours != MONTH_HOURS:
        raise RuntimeError(f"expected {MONTH_HOURS} month-hour bins, got {month_hours}")

    lat_lon = np.array([unpack_location_key(int(key)) for key in location_keys], dtype=np.float32)
    location_index = np.repeat(np.arange(location_count, dtype=np.int32), MONTH_HOURS)
    month_hour = np.tile(np.arange(MONTH_HOURS, dtype=np.int32), location_count)
    lat = lat_lon[location_index, 0]
    lon = lat_lon[location_index, 1]
    month = month_hour // 24
    hour = month_hour % 24
    features = encode_features(lat, lon, month, hour)
    targets = np.concatenate([means.reshape(-1, 5), stds.reshape(-1, 5)], axis=1).astype(np.float32)
    valid = counts.reshape(-1) > 0
    if not valid.all():
        features = features[valid]
        targets = targets[valid]
        location_index = location_index[valid]
        month_hour = month_hour[valid]
    return features, targets, {
        "location_count": location_count,
        "month_hours": month_hours,
        "location_keys": location_keys,
        "location_index": location_index,
        "month_hour": month_hour,
        "valid_rows": int(valid.sum()),
    }


def split_indices(location_index: np.ndarray, stride: int, fit_all: bool) -> tuple[np.ndarray, np.ndarray]:
    all_indices = np.arange(len(location_index), dtype=np.int64)
    if fit_all:
        return all_indices, all_indices
    is_val = location_index % stride == 0
    return all_indices[~is_val], all_indices[is_val]


def encode_features(lat: np.ndarray, lon: np.ndarray, month: np.ndarray, hour: np.ndarray) -> np.ndarray:
    lat_norm = lat.astype(np.float64) / 90.0
    lon_norm = lon.astype(np.float64) / 180.0
    month_angle = 2.0 * math.pi * (month.astype(np.float64) + 0.5) / 12.0
    hour_angle = 2.0 * math.pi * hour.astype(np.float64) / 24.0
    columns: list[np.ndarray] = [lat_norm, lon_norm]
    for harmonic in range(1, 9):
        columns.extend([np.sin(math.pi * lat_norm * harmonic), np.cos(math.pi * lat_norm * harmonic)])
    for harmonic in range(1, 9):
        columns.extend([np.sin(math.pi * lon_norm * harmonic), np.cos(math.pi * lon_norm * harmonic)])
    for harmonic in range(1, 7):
        columns.extend([np.sin(month_angle * harmonic), np.cos(month_angle * harmonic)])
    for harmonic in range(1, 7):
        columns.extend([np.sin(hour_angle * harmonic), np.cos(hour_angle * harmonic)])
    month_sin = np.sin(month_angle)
    month_cos = np.cos(month_angle)
    hour_sin = np.sin(hour_angle)
    hour_cos = np.cos(hour_angle)
    columns.extend([
        lat_norm * month_sin,
        lat_norm * month_cos,
        lon_norm * month_sin,
        lon_norm * month_cos,
        month_sin * hour_sin,
        month_sin * hour_cos,
        month_cos * hour_sin,
        month_cos * hour_cos,
    ])
    output = np.stack(columns, axis=1).astype(np.float32)
    if output.shape[1] != INPUT_FEATURES:
        raise RuntimeError(f"expected {INPUT_FEATURES} features, got {output.shape[1]}")
    return output


def unpack_location_key(key: int) -> tuple[float, float]:
    lat_key = ((key >> 32) & 0xFFFFFFFF) - 90_000
    lon_key = (key & 0xFFFFFFFF) - 180_000
    return lat_key / 1000.0, lon_key / 1000.0


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


class ClimateNormalMlp(nn.Module):
    def __init__(self, input_features: int, width: int, blocks: int, residual_scale: float) -> None:
        super().__init__()
        self.input = nn.Linear(input_features, width)
        self.blocks = nn.Sequential(*(ResidualBlock(width, residual_scale) for _ in range(blocks)))
        self.output = nn.Sequential(nn.LayerNorm(width), nn.SiLU(), nn.Linear(width, TARGETS))

    def forward(self, values: torch.Tensor) -> torch.Tensor:
        hidden = self.input(values)
        hidden = self.blocks(hidden)
        return self.output(hidden)


def evaluate(
    model: ClimateNormalMlp,
    val_x: torch.Tensor,
    val_y: np.ndarray,
    target_mean: np.ndarray,
    target_std: np.ndarray,
    device: torch.device,
) -> dict[str, object]:
    model.eval()
    predictions: list[np.ndarray] = []
    with torch.no_grad():
        for start in range(0, len(val_x), 262144):
            batch = val_x[start : start + 262144]
            if batch.device != device:
                batch = batch.to(device, non_blocking=True)
            pred = model(batch).detach().cpu().numpy()
            predictions.append(pred)
    pred = np.concatenate(predictions, axis=0)
    pred = pred * target_std.reshape(1, TARGETS) + target_mean.reshape(1, TARGETS)
    errors = pred - val_y
    mae = np.mean(np.abs(errors), axis=0)
    rmse = np.sqrt(np.mean(errors * errors, axis=0))
    return {
        "mae": [float(value) for value in mae],
        "rmse": [float(value) for value in rmse],
        "mae_mean": float(np.mean(mae)),
        "rmse_mean": float(np.mean(rmse)),
    }


def save_model(
    path: Path,
    model: ClimateNormalMlp,
    args: argparse.Namespace,
    target_mean: np.ndarray,
    target_std: np.ndarray,
    row_meta: dict[str, object],
    parameters: int,
    epoch: int,
    metrics: dict[str, object],
) -> None:
    torch.save(
        {
            "model_state_dict": model.state_dict(),
            "input_features": INPUT_FEATURES,
            "target_names": TARGET_NAMES,
            "target_mean": target_mean,
            "target_std": target_std,
            "hidden_width": args.hidden_width,
            "residual_blocks": args.residual_blocks,
            "residual_scale": args.residual_scale,
            "parameters": parameters,
            "epoch": epoch,
            "metrics": metrics,
            "row_meta": serializable_meta(row_meta),
        },
        path,
    )


def serializable_meta(row_meta: dict[str, object]) -> dict[str, object]:
    return {
        "location_count": int(row_meta["location_count"]),
        "month_hours": int(row_meta["month_hours"]),
        "valid_rows": int(row_meta["valid_rows"]),
    }


if __name__ == "__main__":
    raise SystemExit(main())
