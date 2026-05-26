#!/usr/bin/env python3
"""Generate deterministic interpolation points between global grid locations."""

from __future__ import annotations

import argparse
import csv
from pathlib import Path


def read_grid(path: Path) -> tuple[list[float], list[float]]:
    latitudes: set[float] = set()
    longitudes: set[float] = set()
    with path.open(newline="", encoding="utf-8") as handle:
        for row in csv.DictReader(handle):
            latitudes.add(float(row["latitude"]))
            longitudes.add(float(row["longitude"]))
    return sorted(latitudes), sorted(longitudes)


def midpoints(values: list[float]) -> list[float]:
    return [(left + right) / 2.0 for left, right in zip(values, values[1:])]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--grid", type=Path, default=Path("experiments/ml-weather/config/global_grid_408_locations.csv"))
    parser.add_argument("--out", type=Path, default=Path("experiments/ml-weather/config/global_grid_368_interpolation_locations.csv"))
    args = parser.parse_args()

    latitudes, longitudes = read_grid(args.grid)
    interp_latitudes = midpoints(latitudes)
    interp_longitudes = midpoints(longitudes)

    args.out.parent.mkdir(parents=True, exist_ok=True)
    with args.out.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(
            handle,
            fieldnames=["location_id", "name", "latitude", "longitude", "region"],
            lineterminator="\n",
        )
        writer.writeheader()
        count = 0
        for lat_index, latitude in enumerate(interp_latitudes):
            for lon_index, longitude in enumerate(interp_longitudes):
                count += 1
                hemisphere = "n" if latitude >= 0 else "s"
                writer.writerow(
                    {
                        "location_id": f"interp_{count:04d}",
                        "name": f"Interpolation grid {count:04d}",
                        "latitude": f"{latitude:.6f}",
                        "longitude": f"{longitude:.6f}",
                        "region": f"interpolation_{hemisphere}_lat{lat_index:02d}_lon{lon_index:02d}",
                    }
                )

    print(f"wrote {args.out}")
    print(f"locations={count}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
