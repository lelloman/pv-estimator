#!/usr/bin/env python3
"""Build source-specific location lists from coverage probe manifests."""

from __future__ import annotations

import argparse
import csv
import json
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

SOURCE_TO_FILENAME = {
    "PVGIS-ERA5": "pvgis_era5_locations.csv",
    "PVGIS-SARAH3": "pvgis_sarah3_locations.csv",
    "PVGIS-NSRDB": "pvgis_nsrdb_locations.csv",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--locations", type=Path, default=Path("experiments/ml-weather/config/source_ensemble_locations_2000.csv"))
    parser.add_argument("--manifest", type=Path, action="append", required=True)
    parser.add_argument("--out-dir", type=Path, default=Path("experiments/ml-weather/config/source_coverage"))
    parser.add_argument("--nsrdb-americas-out", type=Path, default=Path("experiments/ml-weather/config/source_coverage/nsrdb_direct_americas_locations.csv"))
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    locations = read_locations(args.locations)
    location_by_id = {row["location_id"]: row for row in locations}
    covered: dict[str, set[str]] = defaultdict(set)
    status_counts: dict[str, Counter[str]] = defaultdict(Counter)

    for manifest_path in args.manifest:
        data = json.loads(manifest_path.read_text(encoding="utf-8"))
        for record in data.get("files", []):
            database = str(record["database"])
            status = str(record["status"])
            location_id = str(record["location_id"])
            status_counts[database][status] += 1
            if status in {"downloaded", "skipped_existing"}:
                covered[database].add(location_id)

    args.out_dir.mkdir(parents=True, exist_ok=True)
    for source, filename in SOURCE_TO_FILENAME.items():
        rows = [location_by_id[location_id] for location_id in sorted(covered.get(source, set())) if location_id in location_by_id]
        write_locations(args.out_dir / filename, rows)
        print(f"wrote {args.out_dir / filename} rows={len(rows)}")

    americas = [row for row in locations if row.get("region") == "americas"]
    write_locations(args.nsrdb_americas_out, americas)
    print(f"wrote {args.nsrdb_americas_out} rows={len(americas)}")

    summary = {
        "locations": len(locations),
        "status_counts": {source: dict(counter) for source, counter in sorted(status_counts.items())},
        "covered_counts": {source: len(ids) for source, ids in sorted(covered.items())},
        "nsrdb_direct_americas_count": len(americas),
    }
    summary_path = args.out_dir / "coverage_lists.summary.json"
    summary_path.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {summary_path}")
    return 0


def read_locations(path: Path) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def write_locations(path: Path, rows: list[dict[str, str]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if not rows:
        fieldnames = [
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
    else:
        fieldnames = list(rows[0].keys())
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)


if __name__ == "__main__":
    raise SystemExit(main())
