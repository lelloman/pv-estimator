#!/usr/bin/env python3
"""Build an empirical row-interval SARAH3 applicability mask from PVGIS manifests."""

from __future__ import annotations

import argparse
import json
from collections import deque
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

COVERED_STATUSES = {"downloaded", "skipped_existing", "coverage_available"}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, action="append", required=True)
    parser.add_argument("--source", default="PVGIS-SARAH3")
    parser.add_argument("--out", type=Path, default=Path("experiments/ml-weather/config/source_coverage/pvgis_sarah3_empirical_grid_mask_v2.json"))
    parser.add_argument("--grid-step-degrees", type=float, default=1.0)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    centers = covered_centers(args.manifest, args.source)
    if not centers:
        raise SystemExit(f"no covered centers found for {args.source}")
    rows = mask_rows(centers, args.grid_step_degrees)
    payload = {
        "source": args.source,
        "basis": "Empirical applicability mask built from PVGIS coverage manifests.",
        "official_coverage_summary": "PVGIS documents SARAH3 as covering Europe, Africa, most of Asia, and parts of South America. This file is a project operational mask, not an official coverage polygon.",
        "grid_step_degrees": args.grid_step_degrees,
        "half_cell_degrees": args.grid_step_degrees / 2.0,
        "covered_centers": len(centers),
        "created_at_utc": datetime.now(timezone.utc).isoformat(),
        "membership_rule": "A coordinate is inside this empirical mask when its latitude falls in one row lat_min..lat_max and its longitude falls in one of that row's lon_intervals.",
        "connected_components": connected_components(centers, args.grid_step_degrees),
        "rows": rows,
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {args.out} covered_centers={len(centers)} rows={len(rows)}")
    return 0


def covered_centers(paths: list[Path], source: str) -> set[tuple[float, float]]:
    centers: set[tuple[float, float]] = set()
    for path in paths:
        data = json.loads(path.read_text(encoding="utf-8"))
        for record in data.get("files", []):
            if record.get("database") != source:
                continue
            if record.get("status") not in COVERED_STATUSES:
                continue
            centers.add((round(float(record["latitude"]), 6), round(float(record["longitude"]), 6)))
    return centers


def mask_rows(centers: set[tuple[float, float]], step: float) -> list[dict[str, Any]]:
    half = step / 2.0
    by_lat: dict[float, list[float]] = {}
    for lat, lon in centers:
        by_lat.setdefault(lat, []).append(lon)
    rows = []
    for lat in sorted(by_lat):
        intervals = merge_intervals([(lon - half, lon + half) for lon in sorted(by_lat[lat])])
        rows.append({
            "lat_center": lat,
            "lat_min": lat - half,
            "lat_max": lat + half,
            "lon_intervals": intervals,
            "covered_centers": len(by_lat[lat]),
        })
    return rows


def merge_intervals(intervals: list[tuple[float, float]]) -> list[list[float]]:
    if not intervals:
        return []
    merged: list[list[float]] = []
    cur_min, cur_max = intervals[0]
    for lo, hi in intervals[1:]:
        if lo <= cur_max + 1.0e-9:
            cur_max = max(cur_max, hi)
        else:
            merged.append([round(cur_min, 6), round(cur_max, 6)])
            cur_min, cur_max = lo, hi
    merged.append([round(cur_min, 6), round(cur_max, 6)])
    return merged


def connected_components(centers: set[tuple[float, float]], step: float) -> list[dict[str, Any]]:
    remaining = set(centers)
    components = []
    while remaining:
        start = remaining.pop()
        queue = deque([start])
        component = [start]
        while queue:
            lat, lon = queue.popleft()
            for dlat, dlon in [(step, 0.0), (-step, 0.0), (0.0, step), (0.0, -step)]:
                neighbor = (round(lat + dlat, 6), round(lon + dlon, 6))
                if neighbor in remaining:
                    remaining.remove(neighbor)
                    queue.append(neighbor)
                    component.append(neighbor)
        lats = [lat for lat, _lon in component]
        lons = [lon for _lat, lon in component]
        components.append({
            "covered_centers": len(component),
            "lat_min": min(lats) - step / 2.0,
            "lat_max": max(lats) + step / 2.0,
            "lon_min": min(lons) - step / 2.0,
            "lon_max": max(lons) + step / 2.0,
        })
    components.sort(key=lambda item: item["covered_centers"], reverse=True)
    return components


if __name__ == "__main__":
    raise SystemExit(main())
