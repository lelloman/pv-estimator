#!/usr/bin/env python3
"""Download NASA POWER hourly point data for the ML weather experiment."""

from __future__ import annotations

import argparse
import csv
import email.utils
import json
import random
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
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


class RateLimitError(RuntimeError):
    """Raised when NASA POWER asks us to slow down."""

    def __init__(self, message: str, retry_after_seconds: int | None = None) -> None:
        super().__init__(message)
        self.retry_after_seconds = retry_after_seconds


def parse_retry_after(value: str | None) -> int | None:
    if value is None:
        return None
    value = value.strip()
    if value.isdigit():
        return int(value)

    retry_at = email.utils.parsedate_to_datetime(value)
    if retry_at.tzinfo is None:
        retry_at = retry_at.replace(tzinfo=timezone.utc)
    seconds = int((retry_at - datetime.now(timezone.utc)).total_seconds())
    return max(seconds, 0)


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


def fetch_json(url: str, retries: int, timeout_seconds: float) -> bytes:
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
            with urllib.request.urlopen(request, timeout=timeout_seconds) as response:
                return response.read()
        except urllib.error.HTTPError as exc:
            last_error = exc
            if exc.code == 429:
                retry_after = parse_retry_after(exc.headers.get("Retry-After"))
                message = "NASA POWER rate limit returned HTTP 429"
                if retry_after is not None:
                    message = f"{message}; retry after {retry_after} seconds"
                raise RateLimitError(message, retry_after) from exc
            if attempt < retries:
                time.sleep(2.0 * (attempt + 1))
        except (urllib.error.URLError, TimeoutError) as exc:
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
    timeout_seconds: float,
    request_delay_seconds: float,
    request_jitter_seconds: float,
) -> dict[str, Any]:
    location_id = location["location_id"]
    out_path = out_dir / f"{location_id}_{start}_{end}.json"
    url = build_url(location, start, end)

    if out_path.exists() and not force:
        status = "skipped_existing"
        size_bytes = out_path.stat().st_size
    else:
        print(f"[{index}/{total}] downloading {location_id}", flush=True)
        if request_delay_seconds > 0 or request_jitter_seconds > 0:
            time.sleep(request_delay_seconds + random.uniform(0.0, request_jitter_seconds))
        payload = fetch_json(url, retries, timeout_seconds)
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
    parser.add_argument("--timeout-seconds", type=float, default=240.0)
    parser.add_argument("--request-delay-seconds", type=float, default=0.0)
    parser.add_argument("--request-jitter-seconds", type=float, default=0.0)
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

    failure: Exception | None = None
    rate_limited = False

    def add_existing_files_to_manifest() -> None:
        seen = {file["location_id"] for file in manifest["files"]}
        for location in locations:
            location_id = location["location_id"]
            if location_id in seen:
                continue
            out_path = args.out_dir / f"{location_id}_{args.start}_{args.end}.json"
            if not out_path.exists():
                continue
            manifest["files"].append(
                {
                    "location_id": location_id,
                    "name": location["name"],
                    "latitude": float(location["latitude"]),
                    "longitude": float(location["longitude"]),
                    "region": location["region"],
                    "path": str(out_path),
                    "bytes": out_path.stat().st_size,
                    "status": "skipped_existing",
                    "request_url": build_url(location, args.start, args.end),
                }
            )

    def write_manifest(stopped_reason: str | None = None) -> None:
        if stopped_reason is not None:
            add_existing_files_to_manifest()
        manifest["files"].sort(key=lambda file: file["location_id"])
        manifest["completed"] = stopped_reason is None
        if stopped_reason is not None:
            manifest["stopped_reason"] = stopped_reason
        manifest_path = args.out_dir / "manifest.json"
        manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        total_bytes = sum(file["bytes"] for file in manifest["files"])
        downloaded = sum(1 for file in manifest["files"] if file["status"] == "downloaded")
        skipped = sum(1 for file in manifest["files"] if file["status"] == "skipped_existing")
        print(f"wrote {manifest_path}")
        print(f"files={len(manifest['files'])} downloaded={downloaded} skipped={skipped} total_bytes={total_bytes}")

    location_iter = iter(enumerate(locations, start=1))

    def submit_next(executor: ThreadPoolExecutor, pending: dict[Any, None]) -> bool:
        try:
            index, location = next(location_iter)
        except StopIteration:
            return False
        future = executor.submit(
            download_one,
            index,
            len(locations),
            location,
            args.out_dir,
            args.start,
            args.end,
            args.force,
            args.retries,
            args.timeout_seconds,
            args.request_delay_seconds,
            args.request_jitter_seconds,
        )
        pending[future] = None
        return True

    with ThreadPoolExecutor(max_workers=args.workers) as executor:
        pending: dict[Any, None] = {}
        for _ in range(args.workers):
            if not submit_next(executor, pending):
                break

        while pending and failure is None:
            done, _ = wait(pending, return_when=FIRST_COMPLETED)
            for future in done:
                pending.pop(future)
                try:
                    manifest["files"].append(future.result())
                except RateLimitError as exc:
                    failure = exc
                    rate_limited = True
                    break
                except Exception as exc:  # noqa: BLE001 - CLI should report and checkpoint any failure.
                    failure = exc
                    break
                else:
                    submit_next(executor, pending)

            if failure is not None:
                for future in pending:
                    future.cancel()

    if failure is not None:
        write_manifest(str(failure))
        print(f"stopped: {failure}", file=sys.stderr)
        if isinstance(failure, RateLimitError) and failure.retry_after_seconds is not None:
            print(f"retry_after_seconds={failure.retry_after_seconds}", file=sys.stderr)
        return 75 if rate_limited else 1

    write_manifest()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
