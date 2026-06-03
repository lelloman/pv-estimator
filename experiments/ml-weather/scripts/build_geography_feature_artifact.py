#!/usr/bin/env python3
"""Package auxiliary geography CSVs into a runtime feature contract and grid."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path

DEFAULT_EXCLUDE = "location_id,name,latitude,longitude,region,cci_point_class"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--features", type=Path, action="append", required=True, help="Aux geography CSV; pass multiple files to concatenate grids")
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("--exclude-columns", default=DEFAULT_EXCLUDE)
    parser.add_argument("--clip-abs", type=float, default=8.0)
    parser.add_argument("--contract-name", default="feature_contract.json")
    parser.add_argument("--grid-name", default="geography_feature_grid.csv")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    excluded = {item.strip() for item in args.exclude_columns.split(",") if item.strip()}
    rows: list[dict[str, str]] = []
    columns: list[str] | None = None
    seen: set[tuple[int, int]] = set()
    source_hashes = []
    for path in args.features:
        source_hashes.append({"path": str(path), "sha256": sha256_file(path)})
        with path.open(newline="", encoding="utf-8") as handle:
            reader = csv.DictReader(handle)
            if reader.fieldnames is None:
                raise SystemExit(f"{path} has no header")
            file_columns = [column for column in reader.fieldnames if column not in excluded]
            if columns is None:
                columns = numeric_columns(reader, file_columns)
                handle.seek(0)
                reader = csv.DictReader(handle)
            elif columns != [column for column in file_columns if column in columns]:
                missing = [column for column in columns if column not in file_columns]
                if missing:
                    raise SystemExit(f"{path} missing columns required by first feature file: {missing[:5]}")
            for row in reader:
                key = coord_key(float(row["latitude"]), float(row["longitude"]))
                if key in seen:
                    continue
                seen.add(key)
                rows.append(row)
    if columns is None or not columns:
        raise SystemExit("no usable feature columns")
    matrix = [[float(row[column]) for column in columns] for row in rows]
    mean, std = stats(matrix)
    args.out_dir.mkdir(parents=True, exist_ok=True)
    grid_path = args.out_dir / args.grid_name
    with grid_path.open("w", newline="", encoding="utf-8") as handle:
        fieldnames = ["location_id", "latitude", "longitude", "region", *columns]
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        for row in rows:
            writer.writerow({key: row.get(key, "") for key in fieldnames})
    contract = {
        "schema_version": 1,
        "created_at_utc": datetime.now(timezone.utc).isoformat(),
        "columns": columns,
        "mean": mean,
        "std": std,
        "clip_abs": args.clip_abs,
        "grid_path": args.grid_name,
        "grid_rows": len(rows),
        "source_feature_files": source_hashes,
        "grid_sha256": sha256_file(grid_path),
    }
    contract_path = args.out_dir / args.contract_name
    contract_path.write_text(json.dumps(contract, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {contract_path} columns={len(columns)} rows={len(rows)}")
    print(f"wrote {grid_path}")
    return 0


def numeric_columns(reader: csv.DictReader[str], candidates: list[str]) -> list[str]:
    first = next(reader, None)
    if first is None:
        return []
    columns = []
    for column in candidates:
        try:
            float(first[column])
        except (KeyError, ValueError):
            continue
        columns.append(column)
    return columns


def stats(matrix: list[list[float]]) -> tuple[list[float], list[float]]:
    count = len(matrix)
    width = len(matrix[0])
    sums = [0.0] * width
    sumsq = [0.0] * width
    for row in matrix:
        for index, value in enumerate(row):
            sums[index] += value
            sumsq[index] += value * value
    mean = [value / count for value in sums]
    std = []
    for index in range(width):
        variance = max(sumsq[index] / count - mean[index] * mean[index], 1.0e-12)
        std.append(max(variance ** 0.5, 1.0e-6))
    return mean, std


def coord_key(lat: float, lon: float) -> tuple[int, int]:
    return int(round(lat * 1000.0)), int(round(lon * 1000.0))


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


if __name__ == "__main__":
    raise SystemExit(main())
