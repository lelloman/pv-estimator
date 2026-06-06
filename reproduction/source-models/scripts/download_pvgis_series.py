#!/usr/bin/env python3
"""Download PVGIS seriescalc hourly point data for source-model reproduction."""

from __future__ import annotations

import argparse
import csv
import email.utils
import json
import random
import time
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

API_URL = "https://re.jrc.ec.europa.eu/api/v5_3/seriescalc"
DEFAULT_DATABASES = ["PVGIS-ERA5", "PVGIS-SARAH3"]
SOURCE_ID_BY_DATABASE = {
    "PVGIS-ERA5": "pvgis_era5_hourly",
    "PVGIS-SARAH3": "pvgis_sarah3_hourly",
}


class RateLimitError(RuntimeError):
    def __init__(self, message: str, retry_after_seconds: int | None = None) -> None:
        super().__init__(message)
        self.retry_after_seconds = retry_after_seconds


class CoverageError(RuntimeError):
    def __init__(self, message: str, status_code: int, response_body: str) -> None:
        super().__init__(message)
        self.status_code = status_code
        self.response_body = response_body


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--locations", type=Path, default=Path("reproduction/source-models/config/pvgis_benchmark_locations.csv"))
    parser.add_argument("--out-dir", type=Path, default=Path("reproduction/source-models/runs/pvgis_series/raw"))
    parser.add_argument("--databases", default=",".join(DEFAULT_DATABASES))
    parser.add_argument("--start-year", type=int, default=2005)
    parser.add_argument("--end-year", type=int, default=2023)
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("--workers", type=int, default=1)
    parser.add_argument("--retries", type=int, default=3)
    parser.add_argument("--timeout-seconds", type=float, default=240.0)
    parser.add_argument("--request-delay-seconds", type=float, default=2.0)
    parser.add_argument("--request-jitter-seconds", type=float, default=1.0)
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--coverage-only", action="store_true", help="probe source/location availability and write small status records instead of raw hourly payloads")
    return parser.parse_args()


def read_locations(path: Path) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def build_url(location: dict[str, str], database: str, start_year: int, end_year: int) -> str:
    # We request a horizontal plane (angle=0) with components so normalization can
    # recover GHI, DHI, and DNI-like direct normal irradiance consistently.
    query = {
        "lat": location["latitude"],
        "lon": location["longitude"],
        "startyear": start_year,
        "endyear": end_year,
        "raddatabase": database,
        "components": 1,
        "pvcalculation": 0,
        "angle": 0,
        "aspect": 0,
        "usehorizon": 1,
        "outputformat": "json",
    }
    return f"{API_URL}?{urllib.parse.urlencode(query)}"


def parse_retry_after(value: str | None) -> int | None:
    if value is None:
        return None
    value = value.strip()
    if value.isdigit():
        return int(value)
    retry_at = email.utils.parsedate_to_datetime(value)
    if retry_at.tzinfo is None:
        retry_at = retry_at.replace(tzinfo=timezone.utc)
    return max(int((retry_at - datetime.now(timezone.utc)).total_seconds()), 0)


def fetch_json(url: str, retries: int, timeout_seconds: float) -> bytes:
    last_error: Exception | None = None
    for attempt in range(retries + 1):
        try:
            request = urllib.request.Request(
                url,
                headers={
                    "User-Agent": "pv-estimator-pvgis-series-collector/0.1",
                    "Accept": "application/json",
                },
            )
            with urllib.request.urlopen(request, timeout=timeout_seconds) as response:
                return response.read()
        except urllib.error.HTTPError as exc:
            body = exc.read().decode("utf-8", errors="replace")
            last_error = exc
            if exc.code in (429, 529):
                retry_after = parse_retry_after(exc.headers.get("Retry-After"))
                if attempt < retries:
                    time.sleep(float(retry_after if retry_after is not None else 5 * (attempt + 1)))
                    continue
                raise RateLimitError(f"PVGIS returned HTTP {exc.code}", retry_after) from exc
            if exc.code == 400:
                raise CoverageError("PVGIS rejected this source/location request", exc.code, body) from exc
            if attempt < retries:
                time.sleep(3.0 * (attempt + 1))
                continue
        except (urllib.error.URLError, TimeoutError) as exc:
            last_error = exc
            if attempt < retries:
                time.sleep(3.0 * (attempt + 1))
                continue
    raise RuntimeError(f"request failed after {retries + 1} attempts: {last_error}")


def database_dir_name(database: str) -> str:
    return database.lower().replace("-", "_")


def download_one(
    index: int,
    total: int,
    location: dict[str, str],
    database: str,
    out_dir: Path,
    start_year: int,
    end_year: int,
    force: bool,
    retries: int,
    timeout_seconds: float,
    request_delay_seconds: float,
    request_jitter_seconds: float,
    coverage_only: bool,
) -> dict[str, Any]:
    location_id = location["location_id"]
    source_id = SOURCE_ID_BY_DATABASE.get(database, f"pvgis_{database.lower().replace('-', '_')}_hourly")
    source_dir = out_dir / database_dir_name(database)
    source_dir.mkdir(parents=True, exist_ok=True)
    suffix = "coverage.json" if coverage_only else "json"
    out_path = source_dir / f"{location_id}_{start_year}_{end_year}.{suffix}"
    error_path = source_dir / f"{location_id}_{start_year}_{end_year}.error.json"
    url = build_url(location, database, start_year, end_year)

    if out_path.exists() and not force:
        return file_record(location, database, source_id, out_path, "skipped_existing", url)

    if error_path.exists() and not force:
        try:
            error_data = json.loads(error_path.read_text(encoding="utf-8"))
            status = str(error_data.get("status", "skipped_existing_error"))
        except json.JSONDecodeError:
            status = "skipped_existing_error"
        return file_record(location, database, source_id, error_path, status, url)

    print(f"[{index}/{total}] downloading {database} {location_id}", flush=True)
    if request_delay_seconds > 0.0 or request_jitter_seconds > 0.0:
        time.sleep(request_delay_seconds + random.uniform(0.0, request_jitter_seconds))

    try:
        payload = fetch_json(url, retries, timeout_seconds)
    except CoverageError as exc:
        error_payload = {
            "status": "coverage_miss",
            "status_code": exc.status_code,
            "message": str(exc),
            "response_body": exc.response_body,
            "database": database,
            "source_id": source_id,
            "location_id": location_id,
            "latitude": float(location["latitude"]),
            "longitude": float(location["longitude"]),
            "request_url": url,
            "created_at_utc": datetime.now(timezone.utc).isoformat(),
        }
        error_path.write_text(json.dumps(error_payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        return file_record(location, database, source_id, error_path, "coverage_miss", url)

    if coverage_only:
        coverage_payload = {
            "status": "coverage_available",
            "database": database,
            "source_id": source_id,
            "location_id": location_id,
            "latitude": float(location["latitude"]),
            "longitude": float(location["longitude"]),
            "start_year": start_year,
            "end_year": end_year,
            "request_url": url,
            "response_bytes": len(payload),
            "created_at_utc": datetime.now(timezone.utc).isoformat(),
        }
        out_path.write_text(json.dumps(coverage_payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        return file_record(location, database, source_id, out_path, "coverage_available", url)

    out_path.write_bytes(payload)
    return file_record(location, database, source_id, out_path, "downloaded", url)


def file_record(location: dict[str, str], database: str, source_id: str, path: Path, status: str, url: str) -> dict[str, Any]:
    return {
        "location_id": location["location_id"],
        "name": location.get("name", ""),
        "latitude": float(location["latitude"]),
        "longitude": float(location["longitude"]),
        "region": location.get("region", ""),
        "database": database,
        "source_id": source_id,
        "path": str(path),
        "bytes": path.stat().st_size if path.exists() else 0,
        "status": status,
        "request_url": url,
    }


def main() -> int:
    args = parse_args()
    if args.workers <= 0:
        raise SystemExit("--workers must be positive")
    databases = [item.strip() for item in args.databases.split(",") if item.strip()]
    locations = read_locations(args.locations)
    if args.limit is not None:
        locations = locations[: args.limit]
    jobs = [(location, database) for location in locations for database in databases]
    args.out_dir.mkdir(parents=True, exist_ok=True)

    manifest = {
        "source_family": "pvgis_seriescalc",
        "source_url": API_URL,
        "databases": databases,
        "start_year": args.start_year,
        "end_year": args.end_year,
        "created_at_utc": datetime.now(timezone.utc).isoformat(),
        "locations_file": str(args.locations),
        "workers": args.workers,
        "coverage_only": args.coverage_only,
        "request_delay_seconds": args.request_delay_seconds,
        "request_jitter_seconds": args.request_jitter_seconds,
        "files": [],
    }
    failure: Exception | None = None
    rate_limited = False
    job_iter = iter(enumerate(jobs, start=1))

    def submit_next(executor: ThreadPoolExecutor, pending: dict[Any, None]) -> bool:
        try:
            index, (location, database) = next(job_iter)
        except StopIteration:
            return False
        future = executor.submit(
            download_one,
            index,
            len(jobs),
            location,
            database,
            args.out_dir,
            args.start_year,
            args.end_year,
            args.force,
            args.retries,
            args.timeout_seconds,
            args.request_delay_seconds,
            args.request_jitter_seconds,
            args.coverage_only,
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
                except Exception as exc:
                    failure = exc
                    break
                if failure is None:
                    submit_next(executor, pending)

    manifest["files"].sort(key=lambda item: (item["database"], item["location_id"]))
    manifest["completed"] = failure is None
    if failure is not None:
        manifest["stopped_reason"] = str(failure)
        if rate_limited:
            manifest["stopped_kind"] = "rate_limited"
            retry_after = getattr(failure, "retry_after_seconds", None)
            if retry_after is not None:
                manifest["retry_after_seconds"] = retry_after
    manifest_path = args.out_dir / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    statuses: dict[str, int] = {}
    for record in manifest["files"]:
        statuses[record["status"]] = statuses.get(record["status"], 0) + 1
    print(f"wrote {manifest_path}")
    print(f"jobs_recorded={len(manifest['files'])}/{len(jobs)} statuses={statuses}")
    if failure is not None:
        raise SystemExit(2)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
