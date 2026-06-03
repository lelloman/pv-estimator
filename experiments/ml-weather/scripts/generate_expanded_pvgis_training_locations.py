#!/usr/bin/env python3
"""Generate expanded PVGIS training location lists."""

from __future__ import annotations

import argparse
import csv
import hashlib
import math
from collections import defaultdict
from pathlib import Path

DEFAULT_AUX = Path("data/location_features_v1.csv")
DEFAULT_ERA5_EXISTING = Path("config/source_coverage/pvgis_era5_locations.csv")
DEFAULT_SARAH3_EXISTING = Path("config/source_coverage/pvgis_sarah3_locations.csv")
DEFAULT_ERA5_OUT = Path("config/source_coverage/pvgis_era5_expanded_locations_v2.csv")
DEFAULT_SARAH3_OUT = Path("config/source_coverage/pvgis_sarah3_expanded_locations_v2.csv")
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
WEAK_REGION_BOUNDS = {
    "iberia": (35.0, 44.5, -10.5, 4.5),
    "france": (41.0, 51.5, -5.5, 9.5),
    "central_europe": (45.0, 55.5, 4.0, 18.5),
    "balkans_greece_turkey": (35.0, 47.5, 13.0, 36.5),
    "north_africa_middle_east": (18.0, 38.5, -17.5, 55.5),
}
SARAH3_BOUNDS = (-40.0, 65.0, -35.0, 65.0)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--aux-features", type=Path, default=DEFAULT_AUX)
    parser.add_argument("--existing-era5", type=Path, default=DEFAULT_ERA5_EXISTING)
    parser.add_argument("--existing-sarah3", type=Path, default=DEFAULT_SARAH3_EXISTING)
    parser.add_argument("--era5-out", type=Path, default=DEFAULT_ERA5_OUT)
    parser.add_argument("--sarah3-out", type=Path, default=DEFAULT_SARAH3_OUT)
    parser.add_argument("--era5-new-out", type=Path, default=None)
    parser.add_argument("--sarah3-new-out", type=Path, default=None)
    parser.add_argument("--era5-target", type=int, default=6000)
    parser.add_argument("--sarah3-target", type=int, default=3500)
    parser.add_argument("--weak-grid-step-degrees", type=float, default=0.5)
    parser.add_argument("--seed", default="pvgis-expanded-v2")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    aux_rows = [row for row in read_rows(args.aux_features) if is_land_candidate(row)]
    weak_grid_rows = make_weak_region_grid(args.weak_grid_step_degrees)
    candidate_rows = aux_rows + weak_grid_rows
    existing_era5 = read_rows(args.existing_era5)
    existing_sarah3 = read_rows(args.existing_sarah3)

    era5_rows = select_locations(
        candidate_rows,
        existing_era5,
        target=args.era5_target,
        seed=f"{args.seed}:era5",
        sarah3_only=False,
    )
    sarah3_rows = select_locations(
        candidate_rows,
        existing_sarah3,
        target=args.sarah3_target,
        seed=f"{args.seed}:sarah3",
        sarah3_only=True,
    )

    write_rows(args.era5_out, era5_rows)
    write_rows(args.sarah3_out, sarah3_rows)
    if args.era5_new_out is not None:
        era5_existing_keys = {coord_key(row) for row in existing_era5}
        write_rows(args.era5_new_out, [row for row in era5_rows if coord_key(row) not in era5_existing_keys])
    if args.sarah3_new_out is not None:
        sarah3_existing_keys = {coord_key(row) for row in existing_sarah3}
        write_rows(args.sarah3_new_out, [row for row in sarah3_rows if coord_key(row) not in sarah3_existing_keys])
    print(f"wrote {args.era5_out} rows={len(era5_rows)}")
    print_summary("era5", era5_rows)
    print(f"wrote {args.sarah3_out} rows={len(sarah3_rows)}")
    print_summary("sarah3", sarah3_rows)
    return 0


def select_locations(
    aux_rows: list[dict[str, str]],
    existing_rows: list[dict[str, str]],
    target: int,
    seed: str,
    sarah3_only: bool,
) -> list[dict[str, str]]:
    selected: list[dict[str, str]] = []
    seen_keys: set[tuple[int, int]] = set()
    for row in existing_rows:
        selected.append(normalize_existing(row))
        seen_keys.add(coord_key(row))

    candidates = []
    for row in aux_rows:
        if coord_key(row) in seen_keys:
            continue
        lat = float(row["latitude"])
        lon = float(row["longitude"])
        if sarah3_only and not in_bounds(lat, lon, SARAH3_BOUNDS):
            continue
        candidates.append(row)

    target = min(target, len(selected) + len(candidates))
    remaining = target - len(selected)
    if remaining <= 0:
        return selected[:target]

    chosen: list[dict[str, str]] = []
    by_region: dict[str, list[dict[str, str]]] = defaultdict(list)
    for row in candidates:
        by_region[training_region(float(row["latitude"]), float(row["longitude"]))].append(row)

    weak_quota = min(int(remaining * (0.72 if sarah3_only else 0.55)), remaining)
    per_weak_region = max(1, weak_quota // len(WEAK_REGION_BOUNDS))
    for region in WEAK_REGION_BOUNDS:
        pool = sorted(by_region.get(region, []), key=lambda row: candidate_sort_key(row, seed, region))
        take = min(per_weak_region, len(pool), remaining - len(chosen))
        chosen.extend(pool[:take])

    if len(chosen) < remaining:
        chosen_keys = {coord_key(row) for row in chosen}
        rest = [row for row in candidates if coord_key(row) not in chosen_keys]
        rest.sort(key=lambda row: candidate_sort_key(row, seed, training_region(float(row["latitude"]), float(row["longitude"]))))
        chosen.extend(rest[: remaining - len(chosen)])

    for index, row in enumerate(chosen, start=1):
        selected.append(to_output_row(row, f"pvgisx_{index:05d}"))

    selected.sort(key=lambda row: (row["region"], float(row["latitude"]), float(row["longitude"]), row["location_id"]))
    return renumber_generated(selected)


def normalize_existing(row: dict[str, str]) -> dict[str, str]:
    out = {field: row.get(field, "") for field in FIELDNAMES}
    out["latitude"] = f"{float(row['latitude']):.6f}"
    out["longitude"] = f"{float(row['longitude']):.6f}"
    if not out["name"]:
        out["name"] = out["location_id"]
    if not out["split_hint"]:
        out["split_hint"] = split_hint(float(out["latitude"]), float(out["longitude"]))
    return out


def to_output_row(row: dict[str, str], location_id: str) -> dict[str, str]:
    lat = float(row["latitude"])
    lon = float(row["longitude"])
    region = training_region(lat, lon)
    return {
        "location_id": location_id,
        "name": f"Expanded PVGIS {location_id[-5:]}",
        "latitude": f"{lat:.6f}",
        "longitude": f"{lon:.6f}",
        "region": region,
        "split_hint": split_hint(lat, lon),
        "land_context": row.get("land_context") or land_context(row),
        "source_location_id": row["location_id"],
        "land_score": f"{land_score(row):.6f}",
    }


def make_weak_region_grid(step: float) -> list[dict[str, str]]:
    if step <= 0.0:
        raise SystemExit("--weak-grid-step-degrees must be positive")
    rows = []
    for region, bounds in WEAK_REGION_BOUNDS.items():
        min_lat, max_lat, min_lon, max_lon = bounds
        lat = min_lat
        while lat <= max_lat + 1e-9:
            lon = min_lon
            while lon <= max_lon + 1e-9:
                location_id = f"{region}_grid_{round(lat * 100):+05d}_{round(lon * 100):+06d}".replace("+", "p").replace("-", "m")
                rows.append(
                    {
                        "location_id": location_id,
                        "latitude": f"{lat:.6f}",
                        "longitude": f"{lon:.6f}",
                        "region": region,
                        "land_context": "regional_grid_candidate",
                        "regional_grid_score": "0.550000",
                    }
                )
                lon += step
            lat += step
    return rows


def renumber_generated(rows: list[dict[str, str]]) -> list[dict[str, str]]:
    generated = 1
    for row in rows:
        if row["location_id"].startswith("pvgisx_"):
            row["location_id"] = f"pvgisx_{generated:05d}"
            row["name"] = f"Expanded PVGIS {generated:05d}"
            generated += 1
    return rows


def read_rows(path: Path) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def write_rows(path: Path, rows: list[dict[str, str]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=FIELDNAMES)
        writer.writeheader()
        writer.writerows(rows)


def is_land_candidate(row: dict[str, str]) -> bool:
    return land_score(row) > 0.0 and (
        f(row, "etopo_r25km_land_fraction") >= 0.15
        or f(row, "etopo_r100km_land_fraction") >= 0.15
        or f(row, "cci_r25km_water_fraction") < 0.85
        or f(row, "cci_r100km_water_fraction") < 0.85
        or row.get("cci_point_class") != "210"
    )


def training_region(lat: float, lon: float) -> str:
    for region, bounds in WEAK_REGION_BOUNDS.items():
        if in_bounds(lat, lon, bounds):
            return region
    if lon < -30.0:
        return "americas"
    if lon < 60.0:
        if lat >= 30.0:
            return "europe_mediterranean_other"
        if lat >= -35.0:
            return "africa_middle_east_other"
        return "africa_south_atlantic"
    if lat < -10.0:
        return "oceania_south_asia"
    return "asia"


def in_bounds(lat: float, lon: float, bounds: tuple[float, float, float, float]) -> bool:
    min_lat, max_lat, min_lon, max_lon = bounds
    return min_lat <= lat <= max_lat and min_lon <= lon <= max_lon


def split_hint(lat: float, lon: float) -> str:
    lat_tile = math.floor((lat + 90.0) / 10.0)
    lon_tile = math.floor((lon + 180.0) / 10.0)
    value = int((lat_tile * 37 + lon_tile * 17) % 10)
    if value == 0:
        return "test"
    if value == 1:
        return "validation"
    return "train"


def candidate_sort_key(row: dict[str, str], seed: str, region: str) -> tuple[int, int, float, float]:
    lat = float(row["latitude"])
    lon = float(row["longitude"])
    tile = (math.floor(lat * 2.0), math.floor(lon * 2.0))
    return (
        0 if region in WEAK_REGION_BOUNDS else 1,
        stable_int(f"{seed}:{tile[0]}:{tile[1]}"),
        -land_score(row),
        stable_float(f"{seed}:{row['location_id']}"),
    )


def coord_key(row: dict[str, str]) -> tuple[int, int]:
    return (round(float(row["latitude"]) * 1_000_000), round(float(row["longitude"]) * 1_000_000))


def land_score(row: dict[str, str]) -> float:
    if row.get("regional_grid_score"):
        return f(row, "regional_grid_score")
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


def stable_int(payload: str) -> int:
    digest = hashlib.sha256(payload.encode("utf-8")).digest()
    return int.from_bytes(digest[:8], "big")


def stable_float(payload: str) -> float:
    return stable_int(payload) / float(2**64)


def f(row: dict[str, str], key: str) -> float:
    value = row.get(key, "")
    try:
        return float(value)
    except ValueError:
        return 0.0


def print_summary(label: str, rows: list[dict[str, str]]) -> None:
    region_counts: dict[str, int] = defaultdict(int)
    split_counts: dict[str, int] = defaultdict(int)
    for row in rows:
        region_counts[row["region"]] += 1
        split_counts[row["split_hint"]] += 1
    print(f"{label}_regions", dict(sorted(region_counts.items())))
    print(f"{label}_splits", dict(sorted(split_counts.items())))


if __name__ == "__main__":
    raise SystemExit(main())
