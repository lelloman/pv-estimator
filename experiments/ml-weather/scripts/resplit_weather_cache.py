#!/usr/bin/env python3
"""Create harder geography-aware train/validation splits from a sampled weather cache."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np

BASE_INPUTS = 64
TARGETS = 5


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input-cache", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--mode", choices=["spatial-tiles"], default="spatial-tiles")
    parser.add_argument("--tile-degrees", type=float, default=10.0)
    parser.add_argument("--val-limit", type=int, default=1_000_000)
    parser.add_argument("--train-limit", type=int, default=16_000_000)
    parser.add_argument("--seed", type=int, default=7056)
    parser.add_argument("--shuffle", action=argparse.BooleanOptionalAction, default=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.tile_degrees <= 0:
        raise ValueError("--tile-degrees must be positive")
    if args.train_limit <= 0 or args.val_limit <= 0:
        raise ValueError("train and validation limits must be positive")

    print(f"loading {args.input_cache}", flush=True)
    cached = np.load(args.input_cache)
    train_x_old = np.asarray(cached["train_x"], dtype=np.float32)
    train_y_old = np.asarray(cached["train_y"], dtype=np.float32)
    val_x_old = np.asarray(cached["val_x"], dtype=np.float32)
    val_y_old = np.asarray(cached["val_y"], dtype=np.float32)
    validate_cache(train_x_old, train_y_old, val_x_old, val_y_old)

    old_parts = [("old_train", train_x_old, train_y_old), ("old_val", val_x_old, val_y_old)]
    location_counts = count_locations(old_parts)
    val_keys, val_tile_keys = select_spatial_tile_validation_keys(location_counts, args.tile_degrees, args.val_limit, args.seed)
    train_count_available, val_count_available = count_split_rows(old_parts, val_keys)
    train_count = min(args.train_limit, train_count_available)
    val_count = min(args.val_limit, val_count_available)
    if train_count == 0 or val_count == 0:
        raise RuntimeError(f"empty split: train={train_count} val={val_count}")

    print(
        f"split selected_locations={len(val_keys)} selected_tiles={len(val_tile_keys)} "
        f"available_train={train_count_available} available_val={val_count_available} "
        f"writing_train={train_count} writing_val={val_count}",
        flush=True,
    )

    train_x = np.empty((train_count, BASE_INPUTS), dtype=np.float32)
    train_y = np.empty((train_count, TARGETS), dtype=np.float32)
    val_x = np.empty((val_count, BASE_INPUTS), dtype=np.float32)
    val_y = np.empty((val_count, TARGETS), dtype=np.float32)
    fill_outputs(old_parts, val_keys, train_x, train_y, val_x, val_y)

    if args.shuffle:
        rng = np.random.default_rng(args.seed)
        shuffle_pair(train_x, train_y, rng, "train")
        shuffle_pair(val_x, val_y, rng, "val")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    tmp_out = args.out.with_suffix(args.out.suffix + ".tmp")
    metadata = {
        "source_cache": str(args.input_cache),
        "mode": args.mode,
        "tile_degrees": args.tile_degrees,
        "seed": args.seed,
        "selected_validation_locations": len(val_keys),
        "selected_validation_tiles": len(val_tile_keys),
        "available_train_rows": int(train_count_available),
        "available_val_rows": int(val_count_available),
        "train_rows": int(train_count),
        "val_rows": int(val_count),
        "validation_location_keys": [int(key) for key in sorted(val_keys)],
        "validation_tile_keys": [int(key) for key in sorted(val_tile_keys)],
    }
    print(f"writing {args.out}", flush=True)
    with tmp_out.open("wb") as handle:
        np.savez(
            handle,
            train_x=train_x,
            train_y=train_y,
            val_x=val_x,
            val_y=val_y,
            split_metadata=np.array(json.dumps(metadata, sort_keys=True)),
        )
    tmp_out.replace(args.out)
    print(f"wrote {args.out}")
    print(json.dumps({key: value for key, value in metadata.items() if not key.endswith("keys")}, indent=2, sort_keys=True))
    return 0


def validate_cache(train_x: np.ndarray, train_y: np.ndarray, val_x: np.ndarray, val_y: np.ndarray) -> None:
    for name, x, y in [("train", train_x, train_y), ("val", val_x, val_y)]:
        if x.ndim != 2 or x.shape[1] != BASE_INPUTS:
            raise RuntimeError(f"{name}_x must have width {BASE_INPUTS}, got {x.shape}")
        if y.ndim != 2 or y.shape[1] != TARGETS:
            raise RuntimeError(f"{name}_y must have width {TARGETS}, got {y.shape}")
        if len(x) != len(y):
            raise RuntimeError(f"{name} row mismatch: x={len(x)} y={len(y)}")


def count_locations(parts: list[tuple[str, np.ndarray, np.ndarray]]) -> dict[int, int]:
    counts: dict[int, int] = {}
    for name, x, _ in parts:
        keys, row_counts = np.unique(location_keys_for_rows(x), return_counts=True)
        for key, count in zip(keys, row_counts):
            counts[int(key)] = counts.get(int(key), 0) + int(count)
        print(f"counted {name} rows={len(x)} unique_locations={len(keys)}", flush=True)
    print(f"combined unique_locations={len(counts)}", flush=True)
    return counts


def select_spatial_tile_validation_keys(
    location_counts: dict[int, int], tile_degrees: float, val_limit: int, seed: int
) -> tuple[set[int], set[int]]:
    tile_to_locations: dict[int, list[int]] = {}
    for key in location_counts:
        lat, lon = unpack_location_key(key)
        tile_key = spatial_tile_key(lat, lon, tile_degrees)
        tile_to_locations.setdefault(tile_key, []).append(key)

    ordered_tiles = sorted(tile_to_locations, key=lambda tile: stable_hash_u64(tile, seed))
    selected_locations: set[int] = set()
    selected_tiles: set[int] = set()
    selected_rows = 0
    for tile in ordered_tiles:
        selected_tiles.add(tile)
        for location_key in tile_to_locations[tile]:
            if location_key not in selected_locations:
                selected_locations.add(location_key)
                selected_rows += location_counts[location_key]
        if selected_rows >= val_limit:
            break
    return selected_locations, selected_tiles


def count_split_rows(parts: list[tuple[str, np.ndarray, np.ndarray]], val_keys: set[int]) -> tuple[int, int]:
    train_rows = 0
    val_rows = 0
    for _, x, _ in parts:
        mask = np.isin(location_keys_for_rows(x), list(val_keys))
        val_rows += int(mask.sum())
        train_rows += int((~mask).sum())
    return train_rows, val_rows


def fill_outputs(
    parts: list[tuple[str, np.ndarray, np.ndarray]],
    val_keys: set[int],
    train_x: np.ndarray,
    train_y: np.ndarray,
    val_x: np.ndarray,
    val_y: np.ndarray,
) -> None:
    train_offset = 0
    val_offset = 0
    val_key_array = np.array(sorted(val_keys), dtype=np.int64)
    for name, x, y in parts:
        keys = location_keys_for_rows(x)
        is_val = np.isin(keys, val_key_array)
        val_indices = np.flatnonzero(is_val)
        train_indices = np.flatnonzero(~is_val)
        train_offset = copy_rows(x, y, train_indices, train_x, train_y, train_offset, name, "train")
        val_offset = copy_rows(x, y, val_indices, val_x, val_y, val_offset, name, "val")
    print(f"filled train={train_offset}/{len(train_x)} val={val_offset}/{len(val_x)}", flush=True)


def copy_rows(
    src_x: np.ndarray,
    src_y: np.ndarray,
    indices: np.ndarray,
    dst_x: np.ndarray,
    dst_y: np.ndarray,
    offset: int,
    source_name: str,
    split_name: str,
) -> int:
    remaining = len(dst_x) - offset
    if remaining <= 0 or len(indices) == 0:
        return offset
    selected = indices[:remaining]
    end = offset + len(selected)
    dst_x[offset:end] = src_x[selected]
    dst_y[offset:end] = src_y[selected]
    print(f"copied {source_name}->{split_name} rows={len(selected)} offset={end}/{len(dst_x)}", flush=True)
    return end


def shuffle_pair(x: np.ndarray, y: np.ndarray, rng: np.random.Generator, name: str) -> None:
    print(f"shuffling {name} rows={len(x)}", flush=True)
    order = rng.permutation(len(x))
    x[:] = x[order]
    y[:] = y[order]


def location_keys_for_rows(x: np.ndarray) -> np.ndarray:
    lat_keys = np.rint(x[:, 0].astype(np.float64) * 90_000.0).astype(np.int64)
    lon_keys = np.rint(x[:, 1].astype(np.float64) * 180_000.0).astype(np.int64)
    return pack_location_key_arrays(lat_keys, lon_keys)


def pack_location_key_arrays(lat_keys: np.ndarray, lon_keys: np.ndarray) -> np.ndarray:
    return ((lat_keys + 90_000) << 32) | (lon_keys + 180_000)


def unpack_location_key(key: int) -> tuple[float, float]:
    lat_key = ((key >> 32) & 0xFFFFFFFF) - 90_000
    lon_key = (key & 0xFFFFFFFF) - 180_000
    return lat_key / 1000.0, lon_key / 1000.0


def spatial_tile_key(lat: float, lon: float, tile_degrees: float) -> int:
    lat_index = int(np.floor((lat + 90.0) / tile_degrees))
    lon_index = int(np.floor((lon + 180.0) / tile_degrees))
    return (lat_index << 32) | lon_index


def stable_hash_u64(value: int, seed: int) -> int:
    mixed = (value ^ (seed * 0x9E3779B97F4A7C15)) & 0xFFFFFFFFFFFFFFFF
    mixed ^= mixed >> 30
    mixed = (mixed * 0xBF58476D1CE4E5B9) & 0xFFFFFFFFFFFFFFFF
    mixed ^= mixed >> 27
    mixed = (mixed * 0x94D049BB133111EB) & 0xFFFFFFFFFFFFFFFF
    mixed ^= mixed >> 31
    return mixed


if __name__ == "__main__":
    raise SystemExit(main())
