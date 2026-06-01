#!/usr/bin/env python3
"""Generate a land/near-coast PVGIS source-ensemble location set."""

from __future__ import annotations

import argparse
import csv
import hashlib
import math
from collections import defaultdict
from pathlib import Path
from typing import Any

DEFAULT_AUX = Path("experiments/ml-weather/runs/global_grid_7056/aux_geography/location_features_v1.csv")
DEFAULT_OUT = Path("experiments/ml-weather/config/source_ensemble_locations_2000.csv")
FIELDNAMES = [
    "location_id",
    "name",
    "latitude",
    "longitude",
    "region",
    "split_hint",
    "land_context",
    "source_location_id",
    "land_score",
]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--aux-features", type=Path, default=DEFAULT_AUX)
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument("--target", type=int, default=2000)
    parser.add_argument("--seed", default="source-ensemble-v1")
    parser.add_argument("--min-latitude", type=float, default=-60.0)
    parser.add_argument("--max-latitude", type=float, default=65.0)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.target <= 0:
        raise SystemExit("--target must be positive")
    candidates = [row for row in read_rows(args.aux_features) if is_candidate(row, args.min_latitude, args.max_latitude)]
    if len(candidates) < args.target:
        raise SystemExit(f"only {len(candidates)} candidates available for target {args.target}")
    selected = balanced_select(candidates, args.target, args.seed)
    selected.sort(key=lambda row: (float(row["latitude"]), float(row["longitude"])))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with args.out.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=FIELDNAMES)
        writer.writeheader()
        for index, row in enumerate(selected, start=1):
            lat = float(row["latitude"])
            lon = float(row["longitude"])
            out = {
                "location_id": f"srcens_{index:04d}",
                "name": f"Source ensemble {index:04d}",
                "latitude": f"{lat:.6f}",
                "longitude": f"{lon:.6f}",
                "region": region_for(lat, lon),
                "split_hint": split_hint(lat, lon),
                "land_context": land_context(row),
                "source_location_id": row["location_id"],
                "land_score": f"{land_score(row):.6f}",
            }
            writer.writerow(out)
    print(f"wrote {args.out} rows={len(selected)} candidates={len(candidates)}")
    print_summary(selected)
    return 0


def read_rows(path: Path) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def is_candidate(row: dict[str, str], min_latitude: float, max_latitude: float) -> bool:
    lat = float(row["latitude"])
    if lat < min_latitude or lat > max_latitude:
        return False
    if land_score(row) <= 0.0:
        return False
    return (
        f(row, "etopo_r25km_land_fraction") >= 0.15
        or f(row, "etopo_r100km_land_fraction") >= 0.15
        or f(row, "cci_r25km_water_fraction") < 0.85
        or f(row, "cci_r100km_water_fraction") < 0.85
        or row.get("cci_point_class") != "210"
    )


def land_score(row: dict[str, str]) -> float:
    point_land = 0.0 if row.get("cci_point_class") == "210" else 1.0
    etopo25 = f(row, "etopo_r25km_land_fraction")
    etopo100 = f(row, "etopo_r100km_land_fraction")
    cci25 = 1.0 - f(row, "cci_r25km_water_fraction")
    cci100 = 1.0 - f(row, "cci_r100km_water_fraction")
    built = f(row, "cci_r25km_built_fraction")
    crop = f(row, "cci_r25km_cropland_fraction")
    score = 0.30 * point_land + 0.25 * etopo25 + 0.20 * etopo100 + 0.15 * cci25 + 0.05 * cci100 + 0.03 * built + 0.02 * crop
    return max(0.0, min(score, 1.2))


def land_context(row: dict[str, str]) -> str:
    point_water = row.get("cci_point_class") == "210"
    land25 = f(row, "etopo_r25km_land_fraction")
    land100 = f(row, "etopo_r100km_land_fraction")
    water25 = f(row, "cci_r25km_water_fraction")
    water100 = f(row, "cci_r100km_water_fraction")
    if not point_water and land25 >= 0.85 and water25 <= 0.15:
        return "inland_land"
    if not point_water and (0.15 < water100 < 0.85 or land25 < 0.85):
        return "coastal_land"
    if point_water and land100 >= 0.15:
        return "near_coast_water_cell"
    if land100 >= 0.15 or water100 < 0.85:
        return "near_coast"
    return "land_candidate"


def balanced_select(rows: list[dict[str, str]], target: int, seed: str) -> list[dict[str, str]]:
    strata: dict[tuple[str, int], list[dict[str, str]]] = defaultdict(list)
    for row in rows:
        lat = float(row["latitude"])
        lon = float(row["longitude"])
        band = int(math.floor((lat + 90.0) / 15.0))
        strata[(region_for(lat, lon), band)].append(row)

    selected: list[dict[str, str]] = []
    remaining: list[dict[str, str]] = []
    total = len(rows)
    for key in sorted(strata):
        group = sorted(strata[key], key=lambda row: (-land_score(row), stable_noise(row, seed)))
        quota = max(1, round(target * len(group) / total))
        selected.extend(group[:quota])
        remaining.extend(group[quota:])

    if len(selected) > target:
        selected.sort(key=lambda row: (-land_score(row), stable_noise(row, seed)))
        selected = selected[:target]
    elif len(selected) < target:
        remaining.sort(key=lambda row: (-land_score(row), stable_noise(row, seed)))
        selected.extend(remaining[: target - len(selected)])
    return selected


def region_for(lat: float, lon: float) -> str:
    if lon < -30.0:
        return "americas"
    if lon < 60.0:
        if lat >= 30.0:
            return "europe_mediterranean"
        if lat >= -35.0:
            return "africa_middle_east"
        return "africa_south_atlantic"
    if lat < -10.0:
        return "oceania_south_asia"
    return "asia"


def split_hint(lat: float, lon: float) -> str:
    lat_tile = math.floor((lat + 90.0) / 10.0)
    lon_tile = math.floor((lon + 180.0) / 10.0)
    value = int((lat_tile * 37 + lon_tile * 17) % 10)
    if value == 0:
        return "test"
    if value == 1:
        return "validation"
    return "train"


def stable_noise(row: dict[str, str], seed: str) -> float:
    payload = f"{seed}:{row['location_id']}:{row['latitude']}:{row['longitude']}".encode("utf-8")
    digest = hashlib.sha256(payload).digest()
    return int.from_bytes(digest[:8], "big") / float(2**64)


def f(row: dict[str, str], key: str) -> float:
    value = row.get(key, "")
    try:
        return float(value)
    except ValueError:
        return 0.0


def print_summary(rows: list[dict[str, str]]) -> None:
    region_counts: dict[str, int] = defaultdict(int)
    context_counts: dict[str, int] = defaultdict(int)
    split_counts: dict[str, int] = defaultdict(int)
    for row in rows:
        lat = float(row["latitude"])
        lon = float(row["longitude"])
        region_counts[region_for(lat, lon)] += 1
        context_counts[land_context(row)] += 1
        split_counts[split_hint(lat, lon)] += 1
    print("regions", dict(sorted(region_counts.items())))
    print("contexts", dict(sorted(context_counts.items())))
    print("splits", dict(sorted(split_counts.items())))


if __name__ == "__main__":
    raise SystemExit(main())
