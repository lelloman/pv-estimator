#!/usr/bin/env python3
"""Stream-train a climate-normal compressor from large .npy normal tables."""

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
BASE_TARGETS = 5
INPUT_FEATURES = 66
TARGET_NAMES = [
    "ghi_mean_w_m2", "dni_mean_w_m2", "dhi_mean_w_m2", "temp_mean_c", "wind_mean_m_s",
    "ghi_std_w_m2", "dni_std_w_m2", "dhi_std_w_m2", "temp_std_c", "wind_std_m_s",
]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--normals-dir", type=Path, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("--hidden-width", type=int, default=384)
    parser.add_argument("--residual-blocks", type=int, default=6)
    parser.add_argument("--residual-scale", type=float, default=0.5)
    parser.add_argument("--epochs", type=int, default=50)
    parser.add_argument("--batch-size", type=int, default=65536)
    parser.add_argument("--learning-rate", type=float, default=1e-3)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--val-location-stride", type=int, default=17)
    parser.add_argument("--eval-every", type=int, default=1)
    parser.add_argument("--eval-batch-size", type=int, default=262144)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)
    torch.manual_seed(args.seed)
    np.random.seed(args.seed)
    torch.backends.cuda.matmul.allow_tf32 = True
    torch.backends.cudnn.allow_tf32 = True
    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    print(f"device={device}", flush=True)
    if device.type == "cuda":
        print(f"gpu={torch.cuda.get_device_name(0)}", flush=True)

    table = NormalTable.load(args.normals_dir)
    train_locations, val_locations = split_locations(table.location_count, args.val_location_stride)
    print(
        f"locations train={len(train_locations)} val={len(val_locations)} temporal_bins={table.temporal_bins} "
        f"train_rows={len(train_locations) * table.temporal_bins} val_rows={len(val_locations) * table.temporal_bins}",
        flush=True,
    )
    target_mean, target_std = compute_target_stats(table, train_locations)
    model = ClimateNormalMlp(INPUT_FEATURES, args.hidden_width, args.residual_blocks, args.residual_scale).to(device)
    parameters = sum(parameter.numel() for parameter in model.parameters())
    print(f"parameters={parameters}", flush=True)
    optimizer = torch.optim.AdamW(model.parameters(), lr=args.learning_rate)
    rng = np.random.default_rng(args.seed)
    history: list[dict[str, object]] = []
    best_mae_mean = float("inf")
    best_epoch = 0

    for epoch in range(1, args.epochs + 1):
        model.train()
        started = time.time()
        epoch_loss = 0.0
        batches = 0
        location_order = np.array(train_locations, copy=True)
        rng.shuffle(location_order)
        for batch_x, batch_y in iter_batches(table, location_order, args.batch_size, target_mean, target_std):
            x_t = torch.from_numpy(batch_x).to(device)
            y_t = torch.from_numpy(batch_y).to(device)
            optimizer.zero_grad(set_to_none=True)
            pred = model(x_t)
            loss = torch.mean((pred - y_t) ** 2)
            loss.backward()
            optimizer.step()
            epoch_loss += float(loss.detach().cpu())
            batches += 1
            if batches == 1 or batches % 250 == 0:
                elapsed = time.time() - started
                rate = batches / max(elapsed, 0.001)
                print(f"epoch={epoch} batch={batches} rate={rate:.2f}/s", flush=True)
        metrics = evaluate(model, table, val_locations, target_mean, target_std, args.eval_batch_size, device)
        metrics["epoch"] = epoch
        metrics["train_mse_norm"] = epoch_loss / max(batches, 1)
        history.append(metrics)
        if metrics["mae_mean"] < best_mae_mean:
            best_mae_mean = float(metrics["mae_mean"])
            best_epoch = epoch
            save_model(args.out_dir / "best_model.pt", model, args, table, target_mean, target_std, parameters, epoch, metrics)
        print(
            f"epoch={epoch} train_mse_norm={metrics['train_mse_norm']:.6f} "
            f"val_mae_mean={metrics['mae_mean']:.6f} val_mae={metrics['mae']} "
            f"best_epoch={best_epoch} best_mae_mean={best_mae_mean:.6f}",
            flush=True,
        )

    final_metrics = history[-1]
    output = {
        "model": "climate_normals_stream_torch",
        "input_features": INPUT_FEATURES,
        "outputs": TARGETS,
        "target_names": TARGET_NAMES,
        "normals_dir": str(args.normals_dir),
        "temporal_bins": table.temporal_bins,
        "location_count": table.location_count,
        "train_locations": len(train_locations),
        "val_locations": len(val_locations),
        "hidden_width": args.hidden_width,
        "residual_blocks": args.residual_blocks,
        "residual_scale": args.residual_scale,
        "parameters": parameters,
        "model_size_bytes_estimate_float32": parameters * 4,
        "epochs": args.epochs,
        "batch_size": args.batch_size,
        "learning_rate": args.learning_rate,
        "val_location_stride": args.val_location_stride,
        "target_mean": target_mean.tolist(),
        "target_std": target_std.tolist(),
        "device": str(device),
        "gpu": torch.cuda.get_device_name(0) if device.type == "cuda" else None,
        "best_epoch": best_epoch,
        "best_mae_mean": best_mae_mean,
        "history": history,
        **{key: value for key, value in final_metrics.items() if key != "epoch"},
    }
    metrics_path = args.out_dir / "metrics.json"
    metrics_path.write_text(json.dumps(output, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    save_model(args.out_dir / "model.pt", model, args, table, target_mean, target_std, parameters, args.epochs, final_metrics)
    print(f"wrote {metrics_path}", flush=True)
    print(f"wrote {args.out_dir / 'model.pt'}", flush=True)
    print(f"wrote {args.out_dir / 'best_model.pt'}", flush=True)
    return 0


class NormalTable:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.mean = np.load(root / "climate_normals.npy", mmap_mode="r")
        self.std = np.load(root / "climate_normal_std.npy", mmap_mode="r")
        self.count = np.load(root / "climate_normal_counts.npy", mmap_mode="r")
        self.location_keys = np.load(root / "location_keys.npy", mmap_mode="r")
        self.location_count = int(self.mean.shape[0])
        self.temporal_bins = int(self.mean.shape[1])
        self.lat_lon = np.array([unpack_location_key(int(key)) for key in self.location_keys], dtype=np.float32)
        if self.mean.shape != self.std.shape or self.mean.shape[:2] != self.count.shape:
            raise RuntimeError("normal table shape mismatch")

    @classmethod
    def load(cls, root: Path) -> "NormalTable":
        return cls(root)


def split_locations(location_count: int, stride: int) -> tuple[np.ndarray, np.ndarray]:
    loc = np.arange(location_count, dtype=np.int64)
    is_val = loc % stride == 0
    return loc[~is_val], loc[is_val]


def compute_target_stats(table: NormalTable, train_locations: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    sums = np.zeros(TARGETS, dtype=np.float64)
    sumsq = np.zeros(TARGETS, dtype=np.float64)
    count = 0
    for index, loc in enumerate(train_locations, start=1):
        y = np.concatenate([table.mean[loc], table.std[loc]], axis=1).astype(np.float64)
        sums += y.sum(axis=0)
        sumsq += (y * y).sum(axis=0)
        count += len(y)
        if index == 1 or index % 1000 == 0 or index == len(train_locations):
            print(f"stats locations={index}/{len(train_locations)}", flush=True)
    mean = (sums / count).astype(np.float32)
    var = sumsq / count - np.square(sums / count)
    std = np.sqrt(np.maximum(var, 1e-6)).astype(np.float32)
    return mean, np.maximum(std, 1e-3).astype(np.float32)


def iter_batches(
    table: NormalTable,
    location_order: np.ndarray,
    batch_size: int,
    target_mean: np.ndarray,
    target_std: np.ndarray,
):
    temporal = table.temporal_bins
    rows_per_epoch = len(location_order) * temporal
    for start in range(0, rows_per_epoch, batch_size):
        stop = min(start + batch_size, rows_per_epoch)
        local_rows = np.arange(start, stop, dtype=np.int64)
        loc = location_order[local_rows // temporal]
        t = local_rows % temporal
        yield make_batch(table, loc, t, target_mean, target_std)


def make_batch(
    table: NormalTable,
    locations: np.ndarray,
    temporal_indices: np.ndarray,
    target_mean: np.ndarray,
    target_std: np.ndarray,
) -> tuple[np.ndarray, np.ndarray]:
    lat = table.lat_lon[locations, 0]
    lon = table.lat_lon[locations, 1]
    x = encode_features(lat, lon, temporal_indices, table.temporal_bins)
    y = np.concatenate([table.mean[locations, temporal_indices], table.std[locations, temporal_indices]], axis=1).astype(np.float32)
    y = (y - target_mean.reshape(1, TARGETS)) / target_std.reshape(1, TARGETS)
    return x, y.astype(np.float32)


def encode_features(lat: np.ndarray, lon: np.ndarray, temporal_indices: np.ndarray, temporal_bins: int) -> np.ndarray:
    lat_norm = lat.astype(np.float64) / 90.0
    lon_norm = lon.astype(np.float64) / 180.0
    if temporal_bins == 288:
        period = 12.0
        phase = temporal_indices.astype(np.float64) // 24 + 0.5
    elif temporal_bins == 8784:
        period = 366.0
        phase = temporal_indices.astype(np.float64) // 24
    else:
        period = float(temporal_bins // 24)
        phase = temporal_indices.astype(np.float64) // 24
    hour = temporal_indices.astype(np.float64) % 24.0
    season_angle = 2.0 * math.pi * phase / period
    hour_angle = 2.0 * math.pi * hour / 24.0
    columns: list[np.ndarray] = [lat_norm, lon_norm]
    for harmonic in range(1, 9):
        columns.extend([np.sin(math.pi * lat_norm * harmonic), np.cos(math.pi * lat_norm * harmonic)])
    for harmonic in range(1, 9):
        columns.extend([np.sin(math.pi * lon_norm * harmonic), np.cos(math.pi * lon_norm * harmonic)])
    for harmonic in range(1, 7):
        columns.extend([np.sin(season_angle * harmonic), np.cos(season_angle * harmonic)])
    for harmonic in range(1, 7):
        columns.extend([np.sin(hour_angle * harmonic), np.cos(hour_angle * harmonic)])
    season_sin = np.sin(season_angle)
    season_cos = np.cos(season_angle)
    hour_sin = np.sin(hour_angle)
    hour_cos = np.cos(hour_angle)
    columns.extend([
        lat_norm * season_sin,
        lat_norm * season_cos,
        lon_norm * season_sin,
        lon_norm * season_cos,
        season_sin * hour_sin,
        season_sin * hour_cos,
        season_cos * hour_sin,
        season_cos * hour_cos,
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
        self.net = nn.Sequential(nn.LayerNorm(width), nn.Linear(width, width), nn.SiLU(), nn.Linear(width, width))

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
    table: NormalTable,
    val_locations: np.ndarray,
    target_mean: np.ndarray,
    target_std: np.ndarray,
    batch_size: int,
    device: torch.device,
) -> dict[str, object]:
    model.eval()
    abs_error = np.zeros(TARGETS, dtype=np.float64)
    squared_error = np.zeros(TARGETS, dtype=np.float64)
    count = 0
    temporal = table.temporal_bins
    rows = len(val_locations) * temporal
    with torch.no_grad():
        for start in range(0, rows, batch_size):
            stop = min(start + batch_size, rows)
            local_rows = np.arange(start, stop, dtype=np.int64)
            loc = val_locations[local_rows // temporal]
            t = local_rows % temporal
            x, _ = make_batch(table, loc, t, target_mean, target_std)
            y = np.concatenate([table.mean[loc, t], table.std[loc, t]], axis=1).astype(np.float32)
            pred = model(torch.from_numpy(x).to(device)).detach().cpu().numpy()
            pred = pred * target_std.reshape(1, TARGETS) + target_mean.reshape(1, TARGETS)
            errors = pred - y
            abs_error += np.abs(errors).sum(axis=0)
            squared_error += (errors * errors).sum(axis=0)
            count += len(y)
    mae = abs_error / max(count, 1)
    rmse = np.sqrt(squared_error / max(count, 1))
    return {
        "mae": [float(value) for value in mae],
        "rmse": [float(value) for value in rmse],
        "mae_mean": float(np.mean(mae)),
        "rmse_mean": float(np.mean(rmse)),
    }


def save_model(path: Path, model: ClimateNormalMlp, args: argparse.Namespace, table: NormalTable, target_mean: np.ndarray, target_std: np.ndarray, parameters: int, epoch: int, metrics: dict[str, object]) -> None:
    torch.save({
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
        "temporal_bins": table.temporal_bins,
        "location_count": table.location_count,
    }, path)


if __name__ == "__main__":
    raise SystemExit(main())
