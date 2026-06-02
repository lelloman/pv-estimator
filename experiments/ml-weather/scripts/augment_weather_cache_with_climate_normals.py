#!/usr/bin/env python3
"""Append monthly-hourly climate normal features to sampled weather caches."""

from __future__ import annotations

import argparse
import json
import math
from pathlib import Path

import numpy as np

BASE_INPUTS = 64
TARGETS = 5
TARGET_NAMES = [
    "ghi_w_m2",
    "dni_w_m2",
    "dhi_w_m2",
    "ambient_temperature_c",
    "wind_speed_m_s",
]
DAYS_BEFORE_MONTH_COMMON = np.array([0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334, 366])
MONTH_HOURS = 12 * 24


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input-cache", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument(
        "--fit-split",
        choices=["train", "both"],
        default="both",
        help="which rows to use to estimate normals; both is an external-climatology/upper-bound style experiment",
    )
    parser.add_argument("--features", default="mean,std", help="comma-separated: mean,std")
    parser.add_argument("--chunk-size", type=int, default=1_000_000)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    feature_names = [item.strip() for item in args.features.split(",") if item.strip()]
    invalid = sorted(set(feature_names) - {"mean", "std"})
    if invalid:
        raise ValueError(f"unsupported features: {invalid}")
    if not feature_names:
        raise ValueError("at least one feature family is required")

    print(f"loading {args.input_cache}", flush=True)
    cached = np.load(args.input_cache)
    train_x = np.asarray(cached["train_x"], dtype=np.float32)
    train_y = np.asarray(cached["train_y"], dtype=np.float32)
    val_x = np.asarray(cached["val_x"], dtype=np.float32)
    val_y = np.asarray(cached["val_y"], dtype=np.float32)
    validate_cache(train_x, train_y, "train")
    validate_cache(val_x, val_y, "val")

    location_keys = np.unique(np.concatenate([location_keys_for_rows(train_x), location_keys_for_rows(val_x)]))
    location_keys.sort()
    print(f"locations={len(location_keys)}", flush=True)

    fit_parts = [("train", train_x, train_y)]
    if args.fit_split == "both":
        fit_parts.append(("val", val_x, val_y))
    normals = fit_normals(fit_parts, location_keys)
    global_normals = fit_global_normals(fit_parts)

    train_aug = append_normals(train_x, normals, global_normals, location_keys, feature_names, args.chunk_size, "train")
    val_aug = append_normals(val_x, normals, global_normals, location_keys, feature_names, args.chunk_size, "val")

    metadata = {
        "source_cache": str(args.input_cache),
        "fit_split": args.fit_split,
        "features": feature_names,
        "target_names": TARGET_NAMES,
        "location_count": int(len(location_keys)),
        "base_input_features": BASE_INPUTS,
        "climate_normal_features": len(feature_names) * TARGETS,
        "output_input_features": int(train_aug.shape[1]),
        "normal_bins": "location x month x hour",
        "month_from_day_encoding": "common-year month bins inferred from encoded day sin/cos",
    }

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
            climate_normals=normals.mean.astype(np.float32),
            climate_normal_std=normals.std.astype(np.float32),
            climate_normal_counts=normals.count.astype(np.int32),
            location_keys=location_keys.astype(np.int64),
            split_metadata=np.array(json.dumps(metadata, sort_keys=True)),
        )
    tmp_out.replace(args.out)
    print(json.dumps(metadata, indent=2, sort_keys=True), flush=True)
    print(f"wrote {args.out}", flush=True)
    return 0


def validate_cache(x: np.ndarray, y: np.ndarray, name: str) -> None:
    if x.ndim != 2 or x.shape[1] != BASE_INPUTS:
        raise RuntimeError(f"{name}_x must have width {BASE_INPUTS}, got {x.shape}")
    if y.ndim != 2 or y.shape[1] != TARGETS:
        raise RuntimeError(f"{name}_y must have width {TARGETS}, got {y.shape}")
    if len(x) != len(y):
        raise RuntimeError(f"{name} row mismatch: x={len(x)} y={len(y)}")


class Normals:
    def __init__(self, mean: np.ndarray, std: np.ndarray, count: np.ndarray) -> None:
        self.mean = mean
        self.std = std
        self.count = count


def fit_normals(parts: list[tuple[str, np.ndarray, np.ndarray]], location_keys: np.ndarray) -> Normals:
    bins = len(location_keys) * MONTH_HOURS
    count = np.zeros((bins,), dtype=np.int64)
    sums = np.zeros((bins, TARGETS), dtype=np.float64)
    sumsq = np.zeros((bins, TARGETS), dtype=np.float64)
    for name, x, y in parts:
        groups = normal_group_indices(x, location_keys)
        count += np.bincount(groups, minlength=bins)
        for target_index in range(TARGETS):
            values = y[:, target_index].astype(np.float64)
            sums[:, target_index] += np.bincount(groups, weights=values, minlength=bins)
            sumsq[:, target_index] += np.bincount(groups, weights=values * values, minlength=bins)
        print(f"fit normals from {name} rows={len(x)}", flush=True)

    mean = np.zeros_like(sums, dtype=np.float32)
    std = np.ones_like(sums, dtype=np.float32)
    valid = count > 0
    mean[valid] = (sums[valid] / count[valid, None]).astype(np.float32)
    variance = np.zeros_like(sums, dtype=np.float64)
    variance[valid] = sumsq[valid] / count[valid, None] - np.square(sums[valid] / count[valid, None])
    std[valid] = np.sqrt(np.maximum(variance[valid], 1e-6)).astype(np.float32)
    print(f"normal_bins={bins} populated={int(valid.sum())}", flush=True)
    return Normals(mean.reshape(len(location_keys), MONTH_HOURS, TARGETS), std.reshape(len(location_keys), MONTH_HOURS, TARGETS), count.reshape(len(location_keys), MONTH_HOURS))


def fit_global_normals(parts: list[tuple[str, np.ndarray, np.ndarray]]) -> Normals:
    count = np.zeros((MONTH_HOURS,), dtype=np.int64)
    sums = np.zeros((MONTH_HOURS, TARGETS), dtype=np.float64)
    sumsq = np.zeros((MONTH_HOURS, TARGETS), dtype=np.float64)
    for _, x, y in parts:
        groups = month_hour_indices(x)
        count += np.bincount(groups, minlength=MONTH_HOURS)
        for target_index in range(TARGETS):
            values = y[:, target_index].astype(np.float64)
            sums[:, target_index] += np.bincount(groups, weights=values, minlength=MONTH_HOURS)
            sumsq[:, target_index] += np.bincount(groups, weights=values * values, minlength=MONTH_HOURS)
    valid = count > 0
    mean = np.zeros((MONTH_HOURS, TARGETS), dtype=np.float32)
    std = np.ones((MONTH_HOURS, TARGETS), dtype=np.float32)
    mean[valid] = (sums[valid] / count[valid, None]).astype(np.float32)
    variance = np.zeros((MONTH_HOURS, TARGETS), dtype=np.float64)
    variance[valid] = sumsq[valid] / count[valid, None] - np.square(sums[valid] / count[valid, None])
    std[valid] = np.sqrt(np.maximum(variance[valid], 1e-6)).astype(np.float32)
    return Normals(mean.reshape(1, MONTH_HOURS, TARGETS), std.reshape(1, MONTH_HOURS, TARGETS), count.reshape(1, MONTH_HOURS))


def append_normals(
    x: np.ndarray,
    normals: Normals,
    global_normals: Normals,
    location_keys: np.ndarray,
    feature_names: list[str],
    chunk_size: int,
    name: str,
) -> np.ndarray:
    extra_width = len(feature_names) * TARGETS
    output = np.empty((len(x), x.shape[1] + extra_width), dtype=np.float32)
    output[:, : x.shape[1]] = x
    for start in range(0, len(x), chunk_size):
        stop = min(start + chunk_size, len(x))
        loc_indices = location_indices_for_rows(x[start:stop], location_keys)
        mh_indices = month_hour_indices(x[start:stop])
        count = normals.count[loc_indices, mh_indices]
        missing = count == 0
        features: list[np.ndarray] = []
        if "mean" in feature_names:
            mean = normals.mean[loc_indices, mh_indices]
            if missing.any():
                mean[missing] = global_normals.mean[0, mh_indices[missing]]
            features.append(mean)
        if "std" in feature_names:
            std = normals.std[loc_indices, mh_indices]
            if missing.any():
                std[missing] = global_normals.std[0, mh_indices[missing]]
            features.append(std)
        output[start:stop, x.shape[1] :] = np.concatenate(features, axis=1)
        print(f"{name} append_normals {stop}/{len(x)} missing_bins={int(missing.sum())}", flush=True)
    return output


def normal_group_indices(x: np.ndarray, location_keys: np.ndarray) -> np.ndarray:
    return location_indices_for_rows(x, location_keys) * MONTH_HOURS + month_hour_indices(x)


def location_indices_for_rows(x: np.ndarray, location_keys: np.ndarray) -> np.ndarray:
    keys = location_keys_for_rows(x)
    indices = np.searchsorted(location_keys, keys)
    if np.any(indices >= len(location_keys)) or np.any(location_keys[indices] != keys):
        raise KeyError("cache row contains a location missing from location key table")
    return indices.astype(np.int64)


def location_keys_for_rows(x: np.ndarray) -> np.ndarray:
    lat_keys = np.rint(x[:, 0].astype(np.float64) * 90_000.0).astype(np.int64)
    lon_keys = np.rint(x[:, 1].astype(np.float64) * 180_000.0).astype(np.int64)
    return ((lat_keys + 90_000) << 32) | (lon_keys + 180_000)


def month_hour_indices(x: np.ndarray) -> np.ndarray:
    day_angle = np.mod(np.arctan2(x[:, 3].astype(np.float64), x[:, 4].astype(np.float64)), 2.0 * math.pi)
    day_of_year = np.floor(day_angle * 366.0 / (2.0 * math.pi)).astype(np.int32) + 1
    day_of_year = np.clip(day_of_year, 1, 366)
    month = np.searchsorted(DAYS_BEFORE_MONTH_COMMON[1:], day_of_year, side="left").astype(np.int32)

    hour_angle = np.mod(np.arctan2(x[:, 5].astype(np.float64), x[:, 6].astype(np.float64)), 2.0 * math.pi)
    hour = np.rint(hour_angle * 24.0 / (2.0 * math.pi)).astype(np.int32) % 24
    return (month * 24 + hour).astype(np.int64)


if __name__ == "__main__":
    raise SystemExit(main())
