#!/usr/bin/env python3
"""Generate a small high-confidence PVGIS expansion batch."""

from __future__ import annotations

import argparse
import csv
import hashlib
import math
from pathlib import Path

DEFAULT_AUX = Path("data/location_features_v1.csv")
DEFAULT_ERA5_EXISTING = Path("config/source_coverage/pvgis_era5_locations.csv")
DEFAULT_SARAH3_EXISTING = Path("config/source_coverage/pvgis_sarah3_locations.csv")
DEFAULT_BENCHMARKS = [
    Path("config/regional_benchmark_cities_120.csv"),
    Path("config/italy_benchmark_cities_50.csv"),
    Path("config/pvgis_benchmark_locations.csv"),
]
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
SARAH3_BOUNDS = (-40.0, 65.0, -35.0, 65.0)
OFFSETS = [
    (0.0, 0.0),
    (0.18, 0.0),
    (-0.18, 0.0),
    (0.0, 0.18),
    (0.0, -0.18),
    (0.32, 0.24),
    (-0.32, -0.24),
    (0.45, -0.32),
    (-0.45, 0.32),
]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--aux-features", type=Path, default=DEFAULT_AUX)
    parser.add_argument("--existing-era5", type=Path, default=DEFAULT_ERA5_EXISTING)
    parser.add_argument("--existing-sarah3", type=Path, default=DEFAULT_SARAH3_EXISTING)
    parser.add_argument("--benchmark", type=Path, action="append", default=[])
    parser.add_argument("--era5-out", type=Path, required=True)
    parser.add_argument("--sarah3-out", type=Path, required=True)
    parser.add_argument("--era5-target", type=int, default=1000)
    parser.add_argument("--sarah3-target", type=int, default=700)
    parser.add_argument("--seed", default="pvgis-tight-v1")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    benchmarks = args.benchmark or DEFAULT_BENCHMARKS
    existing_era5 = {coord_key(row) for row in read_rows(args.existing_era5)}
    existing_sarah3 = {coord_key(row) for row in read_rows(args.existing_sarah3)}
    candidates = benchmark_candidates(benchmarks)
    candidates.extend(aux_candidates(args.aux_features))
    era5_rows = select(candidates, existing_era5, args.era5_target, args.seed, sarah3_only=False)
    sarah3_rows = select(candidates, existing_sarah3, args.sarah3_target, args.seed, sarah3_only=True)
    write_rows(args.era5_out, era5_rows)
    write_rows(args.sarah3_out, sarah3_rows)
    print(f"wrote {args.era5_out} rows={len(era5_rows)}")
    print_summary("era5", era5_rows)
    print(f"wrote {args.sarah3_out} rows={len(sarah3_rows)}")
    print_summary("sarah3", sarah3_rows)
    return 0


def benchmark_candidates(paths: list[Path]) -> list[dict[str, str]]:
    rows = []
    for path in paths:
        for center in read_rows(path):
            lat0 = float(center["latitude"])
            lon0 = float(center["longitude"])
            for index, (dlat, dlon) in enumerate(OFFSETS):
                lat = max(-60.0, min(65.0, lat0 + dlat))
                lon = normalize_lon(lon0 + dlon)
                rows.append(
                    {
                        "location_id": f"bench_{center['location_id']}_{index:02d}",
                        "name": f"{center.get('name', center['location_id'])} jitter {index:02d}",
                        "latitude": f"{lat:.6f}",
                        "longitude": f"{lon:.6f}",
                        "region": normalize_region(center.get("region", "benchmark")),
                        "split_hint": split_hint(lat, lon),
                        "land_context": "benchmark_jitter",
                        "source_location_id": center["location_id"],
                        "land_score": "0.900000",
                    }
                )
    return rows


def aux_candidates(path: Path) -> list[dict[str, str]]:
    rows = []
    for row in read_rows(path):
        if land_score(row) < 0.80:
            continue
        lat = float(row["latitude"])
        lon = float(row["longitude"])
        rows.append(
            {
                "location_id": f"aux_{row['location_id']}",
                "name": f"Aux land {row['location_id']}",
                "latitude": f"{lat:.6f}",
                "longitude": f"{lon:.6f}",
                "region": broad_region(lat, lon),
                "split_hint": split_hint(lat, lon),
                "land_context": "aux_high_land_score",
                "source_location_id": row["location_id"],
                "land_score": f"{land_score(row):.6f}",
            }
        )
    return rows


def select(candidates: list[dict[str, str]], existing: set[tuple[int, int]], target: int, seed: str, sarah3_only: bool) -> list[dict[str, str]]:
    by_key: dict[tuple[int, int], dict[str, str]] = {}
    for row in candidates:
        lat = float(row["latitude"])
        lon = float(row["longitude"])
        if sarah3_only and not in_sarah3_bounds(lat, lon):
            continue
        key = coord_key(row)
        if key in existing:
            continue
        current = by_key.get(key)
        if current is None or float(row["land_score"]) > float(current["land_score"]):
            by_key[key] = row
    rows = list(by_key.values())
    rows.sort(key=lambda row: sort_key(row, seed))
    rows = rows[:target]
    for index, row in enumerate(rows, start=1):
        row["location_id"] = f"pvgist_{index:05d}"
        row["name"] = f"Tight PVGIS {index:05d}"
    rows.sort(key=lambda row: (row["region"], float(row["latitude"]), float(row["longitude"])))
    return rows


def sort_key(row: dict[str, str], seed: str) -> tuple[int, int, float]:
    priority = 0 if row["land_context"] == "benchmark_jitter" else 1
    return (priority, stable_int(f"{seed}:{row['source_location_id']}:{row['latitude']}:{row['longitude']}"), -float(row["land_score"]))


def read_rows(path: Path) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def write_rows(path: Path, rows: list[dict[str, str]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=FIELDNAMES)
        writer.writeheader()
        writer.writerows(rows)


def coord_key(row: dict[str, str]) -> tuple[int, int]:
    return (round(float(row["latitude"]) * 1_000_000), round(float(row["longitude"]) * 1_000_000))


def split_hint(lat: float, lon: float) -> str:
    lat_tile = math.floor((lat + 90.0) / 10.0)
    lon_tile = math.floor((lon + 180.0) / 10.0)
    value = int((lat_tile * 37 + lon_tile * 17) % 10)
    if value == 0:
        return "test"
    if value == 1:
        return "validation"
    return "train"


def normalize_region(region: str) -> str:
    return "_".join(region.strip().lower().replace(",", " ").split())


def broad_region(lat: float, lon: float) -> str:
    if lon < -30.0:
        return "americas"
    if lon < 60.0:
        return "europe_mediterranean" if lat >= 30.0 else "africa_middle_east"
    return "asia" if lat >= -10.0 else "oceania_south_asia"


def normalize_lon(lon: float) -> float:
    while lon > 180.0:
        lon -= 360.0
    while lon < -180.0:
        lon += 360.0
    return lon


def in_sarah3_bounds(lat: float, lon: float) -> bool:
    min_lat, max_lat, min_lon, max_lon = SARAH3_BOUNDS
    return min_lat <= lat <= max_lat and min_lon <= lon <= max_lon


def land_score(row: dict[str, str]) -> float:
    point_land = 0.0 if row.get("cci_point_class") == "210" else 1.0
    etopo25 = f(row, "etopo_r25km_land_fraction")
    etopo100 = f(row, "etopo_r100km_land_fraction")
    cci25 = 1.0 - f(row, "cci_r25km_water_fraction")
    cci100 = 1.0 - f(row, "cci_r100km_water_fraction")
    built = f(row, "cci_r25km_built_fraction")
    crop = f(row, "cci_r25km_cropland_fraction")
    return max(0.0, min(0.30 * point_land + 0.25 * etopo25 + 0.20 * etopo100 + 0.15 * cci25 + 0.05 * cci100 + 0.03 * built + 0.02 * crop, 1.2))


def stable_int(payload: str) -> int:
    digest = hashlib.sha256(payload.encode("utf-8")).digest()
    return int.from_bytes(digest[:8], "big")


def f(row: dict[str, str], key: str) -> float:
    try:
        return float(row.get(key, ""))
    except ValueError:
        return 0.0


def print_summary(label: str, rows: list[dict[str, str]]) -> None:
    counts: dict[str, int] = {}
    for row in rows:
        counts[row["region"]] = counts.get(row["region"], 0) + 1
    print(f"{label}_regions", dict(sorted(counts.items())))


if __name__ == "__main__":
    raise SystemExit(main())
