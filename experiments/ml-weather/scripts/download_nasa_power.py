#!/usr/bin/env python3
"""Download NASA POWER hourly point data for the ML weather experiment."""

from __future__ import annotations

import argparse
import csv
import json
import time
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

PARAMETERS = [
    "ALLSKY_SFC_SW_DWN",
    "ALLSKY_SFC_SW_DNI",
    "ALLSKY_SFC_SW_DIFF",
    "T2M",
    "WS2M",
]

API_URL = "https://power.larc.nasa.gov/api/temporal/hourly/point"
SOURCE_ID = "nasa_power_hourly"


def read_locations(path: Path) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def build_url(location: dict[str, str], start: str, end: str) -> str:
    query = {
        "parameters": ",".join(PARAMETERS),
        "community": "RE",
        "longitude": location["longitude"],
        "latitude": location["latitude"],
        "start": start,
        "end": end,
        "format": "JSON",
        "time-standard": "UTC",
    }
    return f"{API_URL}?{urllib.parse.urlencode(query)}"


def fetch_json(url: str, retries: int) -> bytes:
    last_error: Exception | None = None
    for attempt in range(retries + 1):
        try:
            request = urllib.request.Request(
                url,
                headers={
                    "User-Agent": "pv-estimator-ml-weather-experiment/0.1",
                    "Accept": "application/json",
                },
            )
            with urllib.request.urlopen(request, timeout=240) as response:
                return response.read()
        except (urllib.error.HTTPError, urllib.error.URLError, TimeoutError) as exc:
            last_error = exc
            if attempt < retries:
                time.sleep(2.0 * (attempt + 1))
    raise RuntimeError(f"request failed after {retries + 1} attempts: {last_error}")


def download_one(
    index: int,
    total: int,
    location: dict[str, str],
    out_dir: Path,
    start: str,
    end: str,
    force: bool,
    retries: int,
) -> dict[str, Any]:
    location_id = location["location_id"]
    out_path = out_dir / f"{location_id}_{start}_{end}.json"
    url = build_url(location, start, end)

    if out_path.exists() and not force:
        status = "skipped_existing"
        size_bytes = out_path.stat().st_size
    else:
        print(f"[{index}/{total}] downloading {location_id}", flush=True)
        payload = fetch_json(url, retries)
        out_path.write_bytes(payload)
        status = "downloaded"
        size_bytes = len(payload)

    return {
        "location_id": location_id,
        "name": location["name"],
        "latitude": float(location["latitude"]),
        "longitude": float(location["longitude"]),
        "region": location["region"],
        "path": str(out_path),
        "bytes": size_bytes,
        "status": status,
        "request_url": url,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--locations", type=Path, default=Path("experiments/ml-weather/config/pilot_locations.csv"))
    parser.add_argument("--out-dir", type=Path, default=Path("experiments/ml-weather/runs/pilot/raw/nasa_power_hourly"))
    parser.add_argument("--start", default="20200101")
    parser.add_argument("--end", default="20241231")
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("--workers", type=int, default=1)
    parser.add_argument("--retries", type=int, default=3)
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args()

    args.out_dir.mkdir(parents=True, exist_ok=True)
    locations = read_locations(args.locations)
    if args.limit is not None:
        locations = locations[: args.limit]

    manifest = {
        "source_id": SOURCE_ID,
        "source_url": API_URL,
        "parameters": PARAMETERS,
        "community": "RE",
        "time_standard": "UTC",
        "start": args.start,
        "end": args.end,
        "created_at_utc": datetime.now(timezone.utc).isoformat(),
        "locations_file": str(args.locations),
        "workers": args.workers,
        "files": [],
    }

    with ThreadPoolExecutor(max_workers=args.workers) as executor:
        futures = [
            executor.submit(
                download_one,
                index,
                len(locations),
                location,
                args.out_dir,
                args.start,
                args.end,
                args.force,
                args.retries,
            )
            for index, location in enumerate(locations, start=1)
        ]
        for future in as_completed(futures):
            manifest["files"].append(future.result())

    manifest["files"].sort(key=lambda file: file["location_id"])
    manifest_path = args.out_dir / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    total_bytes = sum(file["bytes"] for file in manifest["files"])
    downloaded = sum(1 for file in manifest["files"] if file["status"] == "downloaded")
    skipped = sum(1 for file in manifest["files"] if file["status"] == "skipped_existing")
    print(f"wrote {manifest_path}")
    print(f"files={len(manifest['files'])} downloaded={downloaded} skipped={skipped} total_bytes={total_bytes}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
