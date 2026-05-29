#!/usr/bin/env python3
"""Append normalized location-level auxiliary features to cached weather tensors."""

from __future__ import annotations

import argparse
import csv
import hashlib
from pathlib import Path

import numpy as np

BASE_INPUTS = 64


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-cache", type=Path, required=True)
    parser.add_argument("--aux-features", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--train-limit", type=int, default=None)
    parser.add_argument("--val-limit", type=int, default=None)
    parser.add_argument(
        "--aux-exclude-columns",
        default="location_id,name,latitude,longitude,region,cci_point_class",
        help="comma-separated auxiliary CSV columns to exclude from model inputs",
    )
    parser.add_argument("--chunk-size", type=int, default=1_000_000)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    aux = load_aux_features(args.aux_features, args.aux_exclude_columns)
    args.out.parent.mkdir(parents=True, exist_ok=True)

    base = np.load(args.base_cache)
    train_rows = min(args.train_limit or len(base["train_x"]), len(base["train_x"]))
    val_rows = min(args.val_limit or len(base["val_x"]), len(base["val_x"]))
    train_x = append_aux(base["train_x"][:train_rows], aux, args.chunk_size, "train_x")
    val_x = append_aux(base["val_x"][:val_rows], aux, args.chunk_size, "val_x")

    tmp = args.out.with_suffix(args.out.suffix + ".tmp")
    print(f"writing {tmp}", flush=True)
    with tmp.open("wb") as handle:
        np.savez(
            handle,
            train_x=train_x,
            train_y=base["train_y"][:train_rows],
            val_x=val_x,
            val_y=base["val_y"][:val_rows],
        )
    tmp.replace(args.out)
    print(
        f"wrote {args.out} train_x={train_x.shape} val_x={val_x.shape} "
        f"aux_columns={len(aux.columns)} aux_cache_key={aux.cache_key}",
        flush=True,
    )
    return 0


class AuxFeatureSet:
    def __init__(self, columns: list[str], by_coord: dict[tuple[int, int], np.ndarray], cache_key: str) -> None:
        self.columns = columns
        self.by_coord = by_coord
        self.cache_key = cache_key
        self.width = len(columns)

    def for_lat_lon(self, lat: float, lon: float) -> np.ndarray:
        key = coord_key(lat, lon)
        try:
            return self.by_coord[key]
        except KeyError as exc:
            raise KeyError(f"aux features missing lat={lat:.6f} lon={lon:.6f} key={key}") from exc


def load_aux_features(path: Path, exclude_columns: str) -> AuxFeatureSet:
    excluded = {column.strip() for column in exclude_columns.split(",") if column.strip()}
    with path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        if reader.fieldnames is None or "latitude" not in reader.fieldnames or "longitude" not in reader.fieldnames:
            raise ValueError("aux feature CSV must contain latitude and longitude columns")
        columns = [column for column in reader.fieldnames if column not in excluded]
        if not columns:
            raise ValueError("aux feature CSV has no usable columns after exclusions")
        coords: list[tuple[int, int]] = []
        matrix_rows: list[list[float]] = []
        for row in reader:
            coords.append(coord_key(float(row["latitude"]), float(row["longitude"])))
            matrix_rows.append([float(row[column]) for column in columns])

    matrix = np.array(matrix_rows, dtype=np.float32)
    mean = matrix.mean(axis=0).astype(np.float32)
    std = matrix.std(axis=0).astype(np.float32)
    std = np.maximum(std, 1e-6).astype(np.float32)
    normalized = (matrix - mean) / std
    digest = hashlib.sha256()
    digest.update(path.read_bytes())
    digest.update(",".join(columns).encode("utf-8"))
    cache_key = digest.hexdigest()[:12]
    by_coord = {coord: normalized[index] for index, coord in enumerate(coords)}
    print(f"loaded aux rows={len(coords)} columns={len(columns)} cache_key={cache_key}", flush=True)
    return AuxFeatureSet(columns, by_coord, cache_key)


def append_aux(base_x: np.ndarray, aux: AuxFeatureSet, chunk_size: int, name: str) -> np.ndarray:
    if base_x.shape[1] != BASE_INPUTS:
        raise ValueError(f"expected {BASE_INPUTS} base features, got {base_x.shape[1]}")
    out = np.empty((base_x.shape[0], BASE_INPUTS + aux.width), dtype=np.float32)
    for start in range(0, base_x.shape[0], chunk_size):
        end = min(start + chunk_size, base_x.shape[0])
        chunk = base_x[start:end]
        out[start:end, :BASE_INPUTS] = chunk
        for offset, row in enumerate(chunk):
            lat = float(row[0]) * 90.0
            lon = float(row[1]) * 180.0
            out[start + offset, BASE_INPUTS:] = aux.for_lat_lon(lat, lon)
        print(f"{name} {end}/{base_x.shape[0]}", flush=True)
    return out


def coord_key(lat: float, lon: float) -> tuple[int, int]:
    return int(round(lat * 1000.0)), int(round(lon * 1000.0))


if __name__ == "__main__":
    raise SystemExit(main())
