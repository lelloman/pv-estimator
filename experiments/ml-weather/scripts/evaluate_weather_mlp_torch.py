#!/usr/bin/env python3
"""Evaluate a trained weather MLP artifact against a normalized weather CSV."""

from __future__ import annotations

import argparse
import csv
import gzip
import json
import time
from pathlib import Path
from typing import Iterator

import numpy as np
import torch

from train_weather_mlp_torch import (
    INPUTS,
    QUANTILES,
    TARGETS,
    TARGET_NAMES,
    WeatherMlp,
    encode_features,
    parse_timestamp,
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--batch-size", type=int, default=65536)
    parser.add_argument("--limit", type=int, default=None)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    started = time.time()
    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    checkpoint = torch.load(args.model, map_location=device, weights_only=False)
    hidden_layers = checkpoint.get("hidden_layers")
    if hidden_layers is None:
        hidden_width = int(checkpoint["hidden_width"])
        hidden_layers = [hidden_width, hidden_width]

    model = WeatherMlp([int(width) for width in hidden_layers]).to(device)
    model.load_state_dict(checkpoint["model_state_dict"])
    model.eval()

    target_mean = np.asarray(checkpoint["target_mean"], dtype=np.float32)
    target_std = np.asarray(checkpoint["target_std"], dtype=np.float32)
    metrics = StreamingMetrics()

    rows = 0
    with torch.no_grad():
        for batch_x, batch_y in stream_batches(args.data, args.batch_size, args.limit):
            rows += len(batch_x)
            x = torch.from_numpy(batch_x).to(device)
            predictions = model(x).view(-1, TARGETS, 3).detach().cpu().numpy()
            predictions = predictions * target_std.reshape(1, TARGETS, 1) + target_mean.reshape(1, TARGETS, 1)
            metrics.add(batch_y, predictions)
            if rows % 1_000_000 < len(batch_x):
                print(f"evaluated_rows={rows}", flush=True)

    result = metrics.finish()
    result.update(
        {
            "model_path": str(args.model),
            "data_path": str(args.data),
            "rows": rows,
            "target_names": TARGET_NAMES,
            "hidden_layers": hidden_layers,
            "parameters": sum(parameter.numel() for parameter in model.parameters()),
            "device": str(device),
            "gpu": torch.cuda.get_device_name(0) if device.type == "cuda" else None,
            "elapsed_seconds": time.time() - started,
        }
    )
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {args.out}")
    return 0


class StreamingMetrics:
    def __init__(self) -> None:
        self.count = 0
        self.pinball_sum = 0.0
        self.abs_error = np.zeros(TARGETS, dtype=np.float64)
        self.squared_error = np.zeros(TARGETS, dtype=np.float64)
        self.covered = np.zeros(TARGETS, dtype=np.float64)
        self.crossings = 0.0
        self.quantile_sets = 0

    def add(self, actual: np.ndarray, predictions: np.ndarray) -> None:
        crossing_mask = (predictions[:, :, 0] > predictions[:, :, 1]) | (predictions[:, :, 1] > predictions[:, :, 2])
        self.crossings += float(crossing_mask.sum())
        sorted_predictions = np.sort(predictions, axis=2)
        q10 = sorted_predictions[:, :, 0]
        q50 = sorted_predictions[:, :, 1]
        q90 = sorted_predictions[:, :, 2]
        errors = actual - q50
        self.abs_error += np.abs(errors).sum(axis=0)
        self.squared_error += (errors * errors).sum(axis=0)
        self.covered += ((actual >= q10) & (actual <= q90)).sum(axis=0)

        for index, quantile in enumerate([0.1, 0.5, 0.9]):
            q_error = actual - sorted_predictions[:, :, index]
            self.pinball_sum += float(np.maximum(quantile * q_error, (quantile - 1.0) * q_error).sum())

        self.count += len(actual)
        self.quantile_sets += actual.shape[0] * actual.shape[1]

    def finish(self) -> dict[str, object]:
        count = max(self.count, 1)
        return {
            "pinball_loss": self.pinball_sum / max(self.quantile_sets * len(QUANTILES), 1),
            "mae": (self.abs_error / count).tolist(),
            "rmse": np.sqrt(self.squared_error / count).tolist(),
            "coverage_p10_p90": (self.covered / count).tolist(),
            "crossing_rate": self.crossings / max(self.quantile_sets, 1),
        }


def stream_batches(path: Path, batch_size: int, limit: int | None) -> Iterator[tuple[np.ndarray, np.ndarray]]:
    batch_x = np.empty((batch_size, INPUTS), dtype=np.float32)
    batch_y = np.empty((batch_size, TARGETS), dtype=np.float32)
    count = 0
    total = 0
    with gzip.open(path, "rt", newline="", encoding="utf-8") as handle:
        reader = csv.reader(handle)
        next(reader)
        for row in reader:
            if limit is not None and total >= limit:
                break
            timestamp = row[3]
            latitude = float(row[4])
            longitude = float(row[5])
            elevation = float(row[6] or 0.0)
            day_of_year, hour = parse_timestamp(timestamp)
            batch_x[count] = encode_features(latitude, longitude, elevation, day_of_year, hour)
            batch_y[count] = [float(row[7]), float(row[8]), float(row[9]), float(row[10]), float(row[11])]
            count += 1
            total += 1
            if count == batch_size:
                yield batch_x, batch_y
                batch_x = np.empty((batch_size, INPUTS), dtype=np.float32)
                batch_y = np.empty((batch_size, TARGETS), dtype=np.float32)
                count = 0
    if count > 0:
        yield batch_x[:count], batch_y[:count]


if __name__ == "__main__":
    raise SystemExit(main())
