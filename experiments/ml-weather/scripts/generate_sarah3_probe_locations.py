#!/usr/bin/env python3
"""Generate deterministic Europe/Mediterranean SARAH3 coverage probe locations."""

from __future__ import annotations

import argparse
import csv
import math
from pathlib import Path

DEFAULT_AUX = Path("experiments/ml-weather/runs/global_grid_7056/aux_geography/location_features_v1.csv")
DEFAULT_OUT = Path("experiments/ml-weather/runs/sarah3_coverage_probe_v2/locations.csv")
FIELDNAMES = ["location_id", "name", "latitude", "longitude", "region", "split_hint", "land_context"]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--aux-features", type=Path, default=DEFAULT_AUX)
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument("--lat-min", type=float, default=-36.5)
    parser.add_argument("--lat-max", type=float, default=65.5)
    parser.add_argument("--lon-min", type=float, default=-25.0)
    parser.add_argument("--lon-max", type=float, default=70.0)
    parser.add_argument("--step-degrees", type=float, default=1.0)
    parser.add_argument("--min-land-score", type=float, default=0.12)
    parser.add_argument("--extra-locations", type=Path, action="append", default=[])
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.step_degrees <= 0.0:
        raise SystemExit("--step-degrees must be positive")
    aux_rows = read_aux(args.aux_features)
    rows: list[dict[str, str]] = []
    seen: set[tuple[int, int]] = set()
    index = 1
    lat = args.lat_min
    while lat <= args.lat_max + 1.0e-9:
        lon = args.lon_min
        while lon <= args.lon_max + 1.0e-9:
            key = coord_key(lat, lon)
            nearest = nearest_aux(aux_rows, lat, lon)
            if nearest is not None and land_score(nearest) >= args.min_land_score and key not in seen:
                seen.add(key)
                rows.append(row_for(index, lat, lon, "grid"))
                index += 1
            lon += args.step_degrees
        lat += args.step_degrees

    for extra_path in args.extra_locations:
        for extra in read_locations(extra_path):
            lat = float(extra["latitude"])
            lon = float(extra["longitude"])
            key = coord_key(lat, lon)
            if args.lat_min <= lat <= args.lat_max and args.lon_min <= lon <= args.lon_max and key not in seen:
                seen.add(key)
                rows.append(row_for(index, lat, lon, extra.get("location_id", "extra")))
                index += 1

    args.out.parent.mkdir(parents=True, exist_ok=True)
    with args.out.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=FIELDNAMES)
        writer.writeheader()
        writer.writerows(rows)
    print(f"wrote {args.out} rows={len(rows)}")
    return 0


def read_aux(path: Path) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def read_locations(path: Path) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def nearest_aux(rows: list[dict[str, str]], lat: float, lon: float) -> dict[str, str] | None:
    best: tuple[float, dict[str, str]] | None = None
    lat_scale = max(abs(math.cos(math.radians(lat))), 0.1)
    for row in rows:
        row_lat = float(row["latitude"])
        row_lon = float(row["longitude"])
        dist = (lat - row_lat) ** 2 + ((lon - row_lon) * lat_scale) ** 2
        if best is None or dist < best[0]:
            best = (dist, row)
    return None if best is None else best[1]


def land_score(row: dict[str, str]) -> float:
    point_land = 0.0 if row.get("cci_point_class") == "210" else 1.0
    etopo25 = f(row, "etopo_r25km_land_fraction")
    etopo100 = f(row, "etopo_r100km_land_fraction")
    cci25 = 1.0 - f(row, "cci_r25km_water_fraction")
    cci100 = 1.0 - f(row, "cci_r100km_water_fraction")
    return max(0.0, min(1.2, 0.30 * point_land + 0.25 * etopo25 + 0.20 * etopo100 + 0.15 * cci25 + 0.05 * cci100))


def row_for(index: int, lat: float, lon: float, source: str) -> dict[str, str]:
    return {
        "location_id": f"sarah3v2_{index:05d}",
        "name": f"SARAH3 v2 probe {index:05d}",
        "latitude": f"{lat:.6f}",
        "longitude": f"{lon:.6f}",
        "region": region_for(lat, lon),
        "split_hint": source,
        "land_context": "land_or_near_coast",
    }


def region_for(lat: float, lon: float) -> str:
    if lon < -20.0:
        return "atlantic_edge"
    if lat >= 30.0:
        return "europe_mediterranean"
    if lon <= 45.0:
        return "africa_middle_east"
    return "west_asia_edge"


def coord_key(lat: float, lon: float) -> tuple[int, int]:
    return int(round(lat * 1000.0)), int(round(lon * 1000.0))


def f(row: dict[str, str], key: str) -> float:
    try:
        return float(row.get(key, "0") or "0")
    except ValueError:
        return 0.0


if __name__ == "__main__":
    raise SystemExit(main())
