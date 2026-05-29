#!/usr/bin/env python3
"""Download coarse global geography rasters and derive spatial location features."""

from __future__ import annotations

import argparse
import csv
import json
import math
import tempfile
import time
from pathlib import Path
from urllib.request import Request, urlopen

import numpy as np
import rasterio
from rasterio.enums import Resampling
from rasterio.windows import from_bounds

EARTH_RADIUS_KM = 6371.0088
DEFAULT_SOURCES = Path("experiments/ml-weather/config/aux_geography_sources.json")
DEFAULT_SOURCES_DIR = Path("experiments/ml-weather/runs/aux_geography/sources")
DEFAULT_FEATURE_OUT = Path("experiments/ml-weather/runs/global_grid_7056/aux_geography/location_features_v1.csv")

SECTOR_NAMES = ["n", "ne", "e", "se", "s", "sw", "w", "nw"]
CCI_GROUPS: dict[str, set[int]] = {
    "tree": {50, 60, 61, 62, 70, 71, 72, 80, 81, 82, 90},
    "shrub": {120, 121, 122},
    "grass": {110, 130},
    "cropland": {10, 11, 12, 20, 30, 40},
    "wetland": {160, 170, 180},
    "built": {190},
    "bare": {140, 150, 151, 152, 153, 200, 201, 202},
    "water": {210},
    "snow_ice": {220},
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    download = subparsers.add_parser("download")
    download.add_argument("--sources", type=Path, default=DEFAULT_SOURCES)
    download.add_argument("--sources-dir", type=Path, default=DEFAULT_SOURCES_DIR)
    download.add_argument("--force", action="store_true")

    features = subparsers.add_parser("features")
    features.add_argument("--locations", type=Path, default=Path("experiments/ml-weather/config/global_grid_7056_locations.csv"))
    features.add_argument("--sources", type=Path, default=DEFAULT_SOURCES)
    features.add_argument("--sources-dir", type=Path, default=DEFAULT_SOURCES_DIR)
    features.add_argument("--out", type=Path, default=DEFAULT_FEATURE_OUT)
    features.add_argument("--limit", type=int, default=None)
    features.add_argument("--radii-km", default="25,100,250")
    features.add_argument("--patch-pixels", type=int, default=128)
    features.add_argument("--sectors", type=int, default=8)
    features.add_argument("--progress-every", type=int, default=100)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.command == "download":
        download_sources(args)
    elif args.command == "features":
        build_features(args)
    else:
        raise ValueError(f"unknown command: {args.command}")
    return 0


def load_sources(path: Path) -> list[dict[str, object]]:
    data = json.loads(path.read_text(encoding="utf-8"))
    return list(data["sources"])


def download_sources(args: argparse.Namespace) -> None:
    args.sources_dir.mkdir(parents=True, exist_ok=True)
    for source in load_sources(args.sources):
        target = args.sources_dir / str(source["filename"])
        expected_bytes = int(source["expected_bytes"])
        if target.exists() and target.stat().st_size == expected_bytes and not args.force:
            print(f"exists {target} bytes={expected_bytes}")
            continue
        download_file(str(source["url"]), target, expected_bytes)


def download_file(url: str, target: Path, expected_bytes: int) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    request = Request(url, headers={"User-Agent": "pv-estimator-ml-weather/0.1"})
    with urlopen(request, timeout=60) as response:
        total = int(response.headers.get("Content-Length") or expected_bytes)
        with tempfile.NamedTemporaryFile(delete=False, dir=target.parent, prefix=target.name, suffix=".tmp") as tmp:
            tmp_path = Path(tmp.name)
            copied = 0
            started = time.time()
            while True:
                chunk = response.read(1024 * 1024)
                if not chunk:
                    break
                tmp.write(chunk)
                copied += len(chunk)
                if copied == len(chunk) or copied % (64 * 1024 * 1024) < len(chunk):
                    elapsed = max(time.time() - started, 0.001)
                    mib_s = copied / elapsed / 1024 / 1024
                    print(f"downloading {target.name}: {copied}/{total} bytes {mib_s:.1f} MiB/s", flush=True)
    actual = tmp_path.stat().st_size
    if actual != expected_bytes:
        tmp_path.unlink(missing_ok=True)
        raise RuntimeError(f"{target} downloaded {actual} bytes, expected {expected_bytes}")
    tmp_path.replace(target)
    print(f"wrote {target} bytes={actual}")


def build_features(args: argparse.Namespace) -> None:
    radii_km = parse_float_list(args.radii_km)
    if args.sectors != 8:
        raise ValueError("only 8 compass sectors are currently supported")
    sources = {str(source["id"]): args.sources_dir / str(source["filename"]) for source in load_sources(args.sources)}
    locations = read_locations(args.locations)
    if args.limit is not None:
        locations = locations[: args.limit]

    missing = [path for path in sources.values() if not path.exists()]
    if missing:
        missing_text = "\n".join(str(path) for path in missing)
        raise FileNotFoundError(f"missing source rasters; run download first:\n{missing_text}")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    tmp_out = args.out.with_suffix(args.out.suffix + ".tmp")
    started = time.time()
    with rasterio.open(sources["noaa_etopo_2022_60s_bedrock"]) as etopo, rasterio.open(sources["esa_cci_lulc_2020_300m"]) as cci:
        fieldnames = feature_fieldnames(radii_km)
        with tmp_out.open("w", newline="", encoding="utf-8") as out_file:
            writer = csv.DictWriter(out_file, fieldnames=fieldnames)
            writer.writeheader()
            for index, location in enumerate(locations, start=1):
                row = build_location_row(location, etopo, cci, radii_km, args.patch_pixels)
                writer.writerow(row)
                if index == 1 or index % args.progress_every == 0 or index == len(locations):
                    elapsed = time.time() - started
                    rate = index / max(elapsed, 0.001)
                    remaining = (len(locations) - index) / max(rate, 0.001)
                    print(f"features {index}/{len(locations)} rate={rate:.2f}/s eta={remaining/60:.1f}m", flush=True)
    tmp_out.replace(args.out)
    print(f"wrote {args.out}")


def parse_float_list(value: str) -> list[float]:
    result = [float(item.strip()) for item in value.split(",") if item.strip()]
    if not result or any(item <= 0 for item in result):
        raise ValueError("radii must be positive")
    return result


def read_locations(path: Path) -> list[dict[str, object]]:
    with path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        return [
            {
                "location_id": row["location_id"],
                "name": row.get("name", ""),
                "latitude": float(row["latitude"]),
                "longitude": float(row["longitude"]),
                "region": row.get("region", ""),
            }
            for row in reader
        ]


def feature_fieldnames(radii_km: list[float]) -> list[str]:
    fields = ["location_id", "name", "latitude", "longitude", "region"]
    fields.extend(["etopo_point_elevation_m", "cci_point_class"])
    for radius in radii_km:
        prefix = radius_prefix(radius)
        fields.extend(
            [
                f"etopo_{prefix}_elevation_mean_m",
                f"etopo_{prefix}_elevation_std_m",
                f"etopo_{prefix}_elevation_min_m",
                f"etopo_{prefix}_elevation_max_m",
                f"etopo_{prefix}_water_fraction",
                f"etopo_{prefix}_land_fraction",
            ]
        )
        for sector in SECTOR_NAMES:
            fields.extend(
                [
                    f"etopo_{prefix}_sector_{sector}_elevation_mean_m",
                    f"etopo_{prefix}_sector_{sector}_water_fraction",
                ]
            )
        for group in CCI_GROUPS:
            fields.append(f"cci_{prefix}_{group}_fraction")
        for sector in SECTOR_NAMES:
            fields.extend(
                [
                    f"cci_{prefix}_sector_{sector}_water_fraction",
                    f"cci_{prefix}_sector_{sector}_built_fraction",
                    f"cci_{prefix}_sector_{sector}_tree_fraction",
                    f"cci_{prefix}_sector_{sector}_cropland_fraction",
                    f"cci_{prefix}_sector_{sector}_bare_fraction",
                ]
            )
    return fields


def build_location_row(
    location: dict[str, object],
    etopo: rasterio.io.DatasetReader,
    cci: rasterio.io.DatasetReader,
    radii_km: list[float],
    patch_pixels: int,
) -> dict[str, object]:
    lat = float(location["latitude"])
    lon = float(location["longitude"])
    row: dict[str, object] = dict(location)
    row["etopo_point_elevation_m"] = format_float(sample_point(etopo, lon, lat))
    row["cci_point_class"] = int(round(sample_point(cci, lon, lat)))

    for radius in radii_km:
        prefix = radius_prefix(radius)
        etopo_values, lats, lons = read_radius_patch(etopo, lon, lat, radius, patch_pixels, Resampling.bilinear)
        cci_values, cci_lats, cci_lons = read_radius_patch(cci, lon, lat, radius, patch_pixels, Resampling.nearest)
        fill_etopo_features(row, prefix, etopo_values, lats, lons, lat, lon, radius)
        fill_cci_features(row, prefix, cci_values, cci_lats, cci_lons, lat, lon, radius)
    return row


def sample_point(dataset: rasterio.io.DatasetReader, lon: float, lat: float) -> float:
    wrapped_lon = wrap_lon(lon)
    value = next(dataset.sample([(wrapped_lon, lat)], indexes=1, masked=True))
    if np.ma.is_masked(value[0]):
        return float("nan")
    return float(value[0])


def read_radius_patch(
    dataset: rasterio.io.DatasetReader,
    lon: float,
    lat: float,
    radius_km: float,
    patch_pixels: int,
    resampling: Resampling,
) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    lat_delta = radius_km / 111.32
    lon_delta = radius_km / max(111.32 * math.cos(math.radians(lat)), 1.0)
    south = max(lat - lat_delta, -89.999999)
    north = min(lat + lat_delta, 89.999999)
    west = lon - lon_delta
    east = lon + lon_delta

    if west < -180.0:
        left_width = int(round(patch_pixels * (-180.0 - west) / (east - west)))
        right_width = patch_pixels - left_width
        left = read_window(dataset, west + 360.0, 180.0, south, north, left_width, patch_pixels, resampling)
        right = read_window(dataset, -180.0, east, south, north, right_width, patch_pixels, resampling)
        values = np.concatenate([left, right], axis=1)
    elif east > 180.0:
        left_width = int(round(patch_pixels * (180.0 - west) / (east - west)))
        right_width = patch_pixels - left_width
        left = read_window(dataset, west, 180.0, south, north, left_width, patch_pixels, resampling)
        right = read_window(dataset, -180.0, east - 360.0, south, north, right_width, patch_pixels, resampling)
        values = np.concatenate([left, right], axis=1)
    else:
        values = read_window(dataset, west, east, south, north, patch_pixels, patch_pixels, resampling)

    lons = np.linspace(west, east, values.shape[1], endpoint=True)
    lats = np.linspace(north, south, values.shape[0], endpoint=True)
    lon_grid, lat_grid = np.meshgrid(lons, lats)
    lon_grid = wrap_lon_array(lon_grid)
    return values, lat_grid, lon_grid


def read_window(
    dataset: rasterio.io.DatasetReader,
    west: float,
    east: float,
    south: float,
    north: float,
    width: int,
    height: int,
    resampling: Resampling,
) -> np.ndarray:
    width = max(width, 1)
    height = max(height, 1)
    bounds = dataset.bounds
    west = max(west, bounds.left)
    east = min(east, bounds.right)
    south = max(south, bounds.bottom)
    north = min(north, bounds.top)
    window = from_bounds(west, south, east, north, transform=dataset.transform)
    return dataset.read(
        1,
        window=window,
        out_shape=(height, width),
        boundless=True,
        fill_value=dataset.nodata or 0,
        resampling=resampling,
    )


def fill_etopo_features(
    row: dict[str, object],
    prefix: str,
    values: np.ndarray,
    lat_grid: np.ndarray,
    lon_grid: np.ndarray,
    center_lat: float,
    center_lon: float,
    radius_km: float,
) -> None:
    distances = haversine_km(center_lat, center_lon, lat_grid, lon_grid)
    within = distances <= radius_km
    valid = within & np.isfinite(values)
    selected = values[valid].astype(np.float64)
    water = selected <= 0.0
    row[f"etopo_{prefix}_elevation_mean_m"] = format_float(np.mean(selected))
    row[f"etopo_{prefix}_elevation_std_m"] = format_float(np.std(selected))
    row[f"etopo_{prefix}_elevation_min_m"] = format_float(np.min(selected))
    row[f"etopo_{prefix}_elevation_max_m"] = format_float(np.max(selected))
    row[f"etopo_{prefix}_water_fraction"] = format_float(np.mean(water))
    row[f"etopo_{prefix}_land_fraction"] = format_float(1.0 - np.mean(water))

    sectors = sector_indices(center_lat, center_lon, lat_grid, lon_grid)
    for sector_index, sector in enumerate(SECTOR_NAMES):
        sector_mask = valid & (sectors == sector_index)
        sector_values = values[sector_mask].astype(np.float64)
        row[f"etopo_{prefix}_sector_{sector}_elevation_mean_m"] = format_float(np.mean(sector_values))
        row[f"etopo_{prefix}_sector_{sector}_water_fraction"] = format_float(np.mean(sector_values <= 0.0))


def fill_cci_features(
    row: dict[str, object],
    prefix: str,
    values: np.ndarray,
    lat_grid: np.ndarray,
    lon_grid: np.ndarray,
    center_lat: float,
    center_lon: float,
    radius_km: float,
) -> None:
    distances = haversine_km(center_lat, center_lon, lat_grid, lon_grid)
    valid = distances <= radius_km
    selected = values[valid].astype(np.int32)
    for group, classes in CCI_GROUPS.items():
        row[f"cci_{prefix}_{group}_fraction"] = format_float(np.mean(np.isin(selected, list(classes))))

    sectors = sector_indices(center_lat, center_lon, lat_grid, lon_grid)
    sector_groups = ["water", "built", "tree", "cropland", "bare"]
    for sector_index, sector in enumerate(SECTOR_NAMES):
        sector_values = values[valid & (sectors == sector_index)].astype(np.int32)
        for group in sector_groups:
            row[f"cci_{prefix}_sector_{sector}_{group}_fraction"] = format_float(
                np.mean(np.isin(sector_values, list(CCI_GROUPS[group])))
            )


def haversine_km(center_lat: float, center_lon: float, lat_grid: np.ndarray, lon_grid: np.ndarray) -> np.ndarray:
    lat1 = math.radians(center_lat)
    lon1 = math.radians(center_lon)
    lat2 = np.radians(lat_grid)
    lon2 = np.radians(lon_grid)
    dlat = lat2 - lat1
    dlon = (lon2 - lon1 + math.pi) % (2.0 * math.pi) - math.pi
    a = np.sin(dlat / 2.0) ** 2 + math.cos(lat1) * np.cos(lat2) * np.sin(dlon / 2.0) ** 2
    return EARTH_RADIUS_KM * 2.0 * np.arcsin(np.sqrt(a))


def sector_indices(center_lat: float, center_lon: float, lat_grid: np.ndarray, lon_grid: np.ndarray) -> np.ndarray:
    dlon = ((lon_grid - center_lon + 180.0) % 360.0) - 180.0
    x = dlon * np.cos(np.radians(center_lat))
    y = lat_grid - center_lat
    bearings = (np.degrees(np.arctan2(x, y)) + 360.0) % 360.0
    return np.floor(((bearings + 22.5) % 360.0) / 45.0).astype(np.int8)


def radius_prefix(radius: float) -> str:
    if radius.is_integer():
        return f"r{int(radius)}km"
    return f"r{str(radius).replace('.', 'p')}km"


def format_float(value: float | np.floating) -> str:
    if not np.isfinite(value):
        return ""
    return f"{float(value):.6g}"


def wrap_lon(lon: float) -> float:
    return ((lon + 180.0) % 360.0) - 180.0


def wrap_lon_array(lons: np.ndarray) -> np.ndarray:
    return ((lons + 180.0) % 360.0) - 180.0


if __name__ == "__main__":
    raise SystemExit(main())
