#!/usr/bin/env python3
"""Append climate-normal features predicted by a trained compressor model."""

from __future__ import annotations

import argparse
import math
from pathlib import Path

import numpy as np
import torch
from torch import nn

BASE_INPUTS = 64
TARGETS = 10
INPUT_FEATURES = 66
DAYS_BEFORE_MONTH_COMMON = np.array([0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334, 366])


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input-cache", type=Path, required=True)
    parser.add_argument("--compressor", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--train-limit", type=int, default=None)
    parser.add_argument("--val-limit", type=int, default=None)
    parser.add_argument("--chunk-size", type=int, default=1_000_000)
    parser.add_argument("--batch-size", type=int, default=262144)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    print(f"device={device}", flush=True)
    checkpoint = torch.load(args.compressor, map_location=device, weights_only=False)
    model = ClimateNormalMlp(
        int(checkpoint["input_features"]),
        int(checkpoint["hidden_width"]),
        int(checkpoint["residual_blocks"]),
        float(checkpoint["residual_scale"]),
    ).to(device)
    model.load_state_dict(checkpoint["model_state_dict"])
    model.eval()
    target_mean = np.asarray(checkpoint["target_mean"], dtype=np.float32)
    target_std = np.asarray(checkpoint["target_std"], dtype=np.float32)
    temporal_bins = int(checkpoint.get("row_meta", {}).get("month_hours", checkpoint.get("temporal_bins", 288)))
    if temporal_bins != 288:
        raise ValueError(f"expected monthly-hour compressor with 288 temporal bins, got {temporal_bins}")

    print(f"loading {args.input_cache}", flush=True)
    cached = np.load(args.input_cache)
    train_x = np.asarray(cached["train_x"][: args.train_limit], dtype=np.float32)
    train_y = np.asarray(cached["train_y"][: args.train_limit], dtype=np.float32)
    val_x = np.asarray(cached["val_x"][: args.val_limit], dtype=np.float32)
    val_y = np.asarray(cached["val_y"][: args.val_limit], dtype=np.float32)
    validate_cache(train_x, train_y, "train")
    validate_cache(val_x, val_y, "val")

    train_aug = append_predictions(train_x, model, target_mean, target_std, args.chunk_size, args.batch_size, device, "train")
    val_aug = append_predictions(val_x, model, target_mean, target_std, args.chunk_size, args.batch_size, device, "val")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    tmp_out = args.out.with_suffix(args.out.suffix + ".tmp")
    print(f"writing {args.out}", flush=True)
    with tmp_out.open("wb") as handle:
        np.savez(
            handle,
            train_x=train_aug,
            train_y=train_y,
            val_x=val_aug,
            val_y=val_y,
            compressor_path=np.array(str(args.compressor)),
            climate_normal_features=np.array(TARGETS, dtype=np.int32),
        )
    tmp_out.replace(args.out)
    print(f"wrote {args.out}", flush=True)
    return 0


def validate_cache(x: np.ndarray, y: np.ndarray, name: str) -> None:
    if x.ndim != 2 or x.shape[1] != BASE_INPUTS:
        raise RuntimeError(f"{name}_x must have width {BASE_INPUTS}, got {x.shape}")
    if y.ndim != 2 or y.shape[1] != 5:
        raise RuntimeError(f"{name}_y must have width 5, got {y.shape}")


def append_predictions(
    base_x: np.ndarray,
    model: nn.Module,
    target_mean: np.ndarray,
    target_std: np.ndarray,
    chunk_size: int,
    batch_size: int,
    device: torch.device,
    name: str,
) -> np.ndarray:
    output = np.empty((len(base_x), base_x.shape[1] + TARGETS), dtype=np.float32)
    output[:, : base_x.shape[1]] = base_x
    with torch.no_grad():
        for chunk_start in range(0, len(base_x), chunk_size):
            chunk_stop = min(chunk_start + chunk_size, len(base_x))
            chunk = base_x[chunk_start:chunk_stop]
            predicted_parts: list[np.ndarray] = []
            for batch_start in range(0, len(chunk), batch_size):
                batch_stop = min(batch_start + batch_size, len(chunk))
                features = encode_compressor_features(chunk[batch_start:batch_stop])
                pred = model(torch.from_numpy(features).to(device)).detach().cpu().numpy()
                pred = pred * target_std.reshape(1, TARGETS) + target_mean.reshape(1, TARGETS)
                predicted_parts.append(pred.astype(np.float32))
            output[chunk_start:chunk_stop, base_x.shape[1] :] = np.concatenate(predicted_parts, axis=0)
            print(f"{name} predicted_normals {chunk_stop}/{len(base_x)}", flush=True)
    return output


def encode_compressor_features(base_x: np.ndarray) -> np.ndarray:
    lat = base_x[:, 0].astype(np.float64) * 90.0
    lon = base_x[:, 1].astype(np.float64) * 180.0
    day_angle = np.mod(np.arctan2(base_x[:, 3].astype(np.float64), base_x[:, 4].astype(np.float64)), 2.0 * math.pi)
    day_of_year = np.floor(day_angle * 366.0 / (2.0 * math.pi)).astype(np.int32) + 1
    day_of_year = np.clip(day_of_year, 1, 366)
    month = np.searchsorted(DAYS_BEFORE_MONTH_COMMON[1:], day_of_year, side="left").astype(np.float64)
    hour_angle = np.mod(np.arctan2(base_x[:, 5].astype(np.float64), base_x[:, 6].astype(np.float64)), 2.0 * math.pi)
    hour = np.rint(hour_angle * 24.0 / (2.0 * math.pi)).astype(np.int32) % 24
    return encode_features(lat.astype(np.float32), lon.astype(np.float32), month, hour.astype(np.float64))


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


if __name__ == "__main__":
    raise SystemExit(main())
