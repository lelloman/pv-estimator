#!/usr/bin/env python3
"""Generate deterministic non-polar coordinate samples for ML weather training."""

from __future__ import annotations

import argparse
import csv
from pathlib import Path


def frange(start: float, stop: float, step: float) -> list[float]:
    values: list[float] = []
    current = start
    while current <= stop + 1e-9:
        values.append(round(current, 6))
        current += step
    return values


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", type=Path, default=Path("experiments/ml-weather/config/global_grid_408_locations.csv"))
    parser.add_argument("--lat-start", type=float, default=-60.0)
    parser.add_argument("--lat-stop", type=float, default=60.0)
    parser.add_argument("--lat-step", type=float, default=7.5)
    parser.add_argument("--lon-start", type=float, default=-172.5)
    parser.add_argument("--lon-stop", type=float, default=172.5)
    parser.add_argument("--lon-step", type=float, default=15.0)
    args = parser.parse_args()

    latitudes = frange(args.lat_start, args.lat_stop, args.lat_step)
    longitudes = frange(args.lon_start, args.lon_stop, args.lon_step)

    args.out.parent.mkdir(parents=True, exist_ok=True)
    with args.out.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(
            handle,
            fieldnames=["location_id", "name", "latitude", "longitude", "region"],
            lineterminator="\n",
        )
        writer.writeheader()
        count = 0
        for lat_index, latitude in enumerate(latitudes):
            for lon_index, longitude in enumerate(longitudes):
                count += 1
                hemisphere = "n" if latitude >= 0 else "s"
                writer.writerow(
                    {
                        "location_id": f"grid_{count:04d}",
                        "name": f"Global grid {count:04d}",
                        "latitude": f"{latitude:.6f}",
                        "longitude": f"{longitude:.6f}",
                        "region": f"global_{hemisphere}_lat{lat_index:02d}_lon{lon_index:02d}",
                    }
                )

    print(f"wrote {args.out}")
    print(f"locations={count}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
