#!/usr/bin/env python3
"""Download direct NSRDB PSM3 point/year CSV files."""

from __future__ import annotations

import argparse
import csv
import email.utils
import json
import os
import random
import time
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

API_URL = "https://developer.nlr.gov/api/nsrdb/v2/solar/psm3-download.csv"
SOURCE_ID = "nsrdb_psm3_hourly"
DEFAULT_ATTRIBUTES = "ghi,dni,dhi,air_temperature,wind_speed"


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
    parser.add_argument("--locations", type=Path, default=Path("experiments/ml-weather/config/source_coverage/nsrdb_direct_americas_locations.csv"))
    parser.add_argument("--out-dir", type=Path, default=Path("experiments/ml-weather/runs/nsrdb_psm3/raw"))
    parser.add_argument("--start-year", type=int, default=2005)
    parser.add_argument("--end-year", type=int, default=2023)
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("--workers", type=int, default=1)
    parser.add_argument("--retries", type=int, default=3)
    parser.add_argument("--timeout-seconds", type=float, default=240.0)
    parser.add_argument("--request-delay-seconds", type=float, default=1.0)
    parser.add_argument("--request-jitter-seconds", type=float, default=0.5)
    parser.add_argument("--attributes", default=DEFAULT_ATTRIBUTES)
    parser.add_argument("--api-key", default=os.environ.get("NSRDB_API_KEY"))
    parser.add_argument("--full-name", default=os.environ.get("NSRDB_FULL_NAME", "pv-estimator contributor"))
    parser.add_argument("--email", default=os.environ.get("NSRDB_EMAIL"))
    parser.add_argument("--affiliation", default=os.environ.get("NSRDB_AFFILIATION", "pv-estimator"))
    parser.add_argument("--reason", default=os.environ.get("NSRDB_REASON", "research"))
    parser.add_argument("--mailing-list", default=os.environ.get("NSRDB_MAILING_LIST", "false"))
    parser.add_argument("--force", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    validate_args(args)
    locations = read_locations(args.locations)
    if args.limit is not None:
        locations = locations[: args.limit]
    years = list(range(args.start_year, args.end_year + 1))
    jobs = [(location, year) for location in locations for year in years]
    args.out_dir.mkdir(parents=True, exist_ok=True)
    manifest = {
        "source_id": SOURCE_ID,
        "source_url": API_URL,
        "attributes": args.attributes,
        "start_year": args.start_year,
        "end_year": args.end_year,
        "created_at_utc": datetime.now(timezone.utc).isoformat(),
        "locations_file": str(args.locations),
        "workers": args.workers,
        "request_delay_seconds": args.request_delay_seconds,
        "request_jitter_seconds": args.request_jitter_seconds,
        "files": [],
    }
    failure: Exception | None = None
    rate_limited = False
    job_iter = iter(enumerate(jobs, start=1))

    def submit_next(executor: ThreadPoolExecutor, pending: dict[Any, None]) -> bool:
        try:
            index, (location, year) = next(job_iter)
        except StopIteration:
            return False
        future = executor.submit(download_one, index, len(jobs), location, year, args)
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

    manifest["files"].sort(key=lambda item: (item["location_id"], item["year"]))
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


def validate_args(args: argparse.Namespace) -> None:
    if not args.api_key:
        raise SystemExit("missing NSRDB API key; set NSRDB_API_KEY or pass --api-key")
    if not args.email:
        raise SystemExit("missing NSRDB email; set NSRDB_EMAIL or pass --email")
    if args.workers <= 0:
        raise SystemExit("--workers must be positive")
    if args.start_year > args.end_year:
        raise SystemExit("--start-year must be <= --end-year")


def read_locations(path: Path) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def download_one(index: int, total: int, location: dict[str, str], year: int, args: argparse.Namespace) -> dict[str, Any]:
    location_id = location["location_id"]
    out_path = args.out_dir / f"{location_id}_{year}.csv"
    error_path = args.out_dir / f"{location_id}_{year}.error.json"
    url = build_url(location, year, args)
    if out_path.exists() and not args.force:
        return file_record(location, year, out_path, "skipped_existing", url)
    if error_path.exists() and not args.force:
        try:
            status = str(json.loads(error_path.read_text(encoding="utf-8")).get("status", "skipped_existing_error"))
        except json.JSONDecodeError:
            status = "skipped_existing_error"
        return file_record(location, year, error_path, status, url)
    print(f"[{index}/{total}] downloading NSRDB {location_id} {year}", flush=True)
    if args.request_delay_seconds > 0.0 or args.request_jitter_seconds > 0.0:
        time.sleep(args.request_delay_seconds + random.uniform(0.0, args.request_jitter_seconds))
    try:
        payload = fetch(url, args.retries, args.timeout_seconds)
    except CoverageError as exc:
        error_payload = {
            "status": "coverage_miss",
            "status_code": exc.status_code,
            "message": str(exc),
            "response_body": exc.response_body[:4000],
            "source_id": SOURCE_ID,
            "location_id": location_id,
            "year": year,
            "latitude": float(location["latitude"]),
            "longitude": float(location["longitude"]),
            "request_url": redact_api_key(url),
            "created_at_utc": datetime.now(timezone.utc).isoformat(),
        }
        error_path.write_text(json.dumps(error_payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        return file_record(location, year, error_path, "coverage_miss", url)
    out_path.write_bytes(payload)
    return file_record(location, year, out_path, "downloaded", url)


def build_url(location: dict[str, str], year: int, args: argparse.Namespace) -> str:
    wkt = f"POINT({location['longitude']} {location['latitude']})"
    query = {
        "api_key": args.api_key,
        "wkt": wkt,
        "names": str(year),
        "interval": "60",
        "utc": "true",
        "leap_day": "false",
        "attributes": args.attributes,
        "full_name": args.full_name,
        "email": args.email,
        "affiliation": args.affiliation,
        "reason": args.reason,
        "mailing_list": args.mailing_list,
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


def fetch(url: str, retries: int, timeout_seconds: float) -> bytes:
    last_error: Exception | None = None
    for attempt in range(retries + 1):
        try:
            request = urllib.request.Request(url, headers={"User-Agent": "pv-estimator-nsrdb-collector/0.1"})
            with urllib.request.urlopen(request, timeout=timeout_seconds) as response:
                payload = response.read()
                if payload.lstrip().startswith(b"{"):
                    raise CoverageError("NSRDB returned JSON instead of CSV", 200, payload.decode("utf-8", errors="replace"))
                return payload
        except urllib.error.HTTPError as exc:
            body = exc.read().decode("utf-8", errors="replace")
            last_error = exc
            if exc.code in (429, 503):
                retry_after = parse_retry_after(exc.headers.get("Retry-After"))
                if attempt < retries:
                    time.sleep(float(retry_after if retry_after is not None else 5 * (attempt + 1)))
                    continue
                raise RateLimitError(f"NSRDB returned HTTP {exc.code}", retry_after) from exc
            if exc.code in (400, 404, 422):
                raise CoverageError("NSRDB rejected this source/location/year request", exc.code, body) from exc
            if attempt < retries:
                time.sleep(3.0 * (attempt + 1))
                continue
        except (urllib.error.URLError, TimeoutError) as exc:
            last_error = exc
            if attempt < retries:
                time.sleep(3.0 * (attempt + 1))
                continue
    raise RuntimeError(f"request failed after {retries + 1} attempts: {last_error}")


def file_record(location: dict[str, str], year: int, path: Path, status: str, url: str) -> dict[str, Any]:
    return {
        "source_id": SOURCE_ID,
        "location_id": location["location_id"],
        "name": location.get("name", ""),
        "latitude": float(location["latitude"]),
        "longitude": float(location["longitude"]),
        "region": location.get("region", ""),
        "year": year,
        "path": str(path),
        "bytes": path.stat().st_size if path.exists() else 0,
        "status": status,
        "request_url": redact_api_key(url),
    }


def redact_api_key(url: str) -> str:
    parsed = urllib.parse.urlsplit(url)
    query = urllib.parse.parse_qsl(parsed.query, keep_blank_values=True)
    redacted = [(key, "REDACTED" if key == "api_key" else value) for key, value in query]
    return urllib.parse.urlunsplit((parsed.scheme, parsed.netloc, parsed.path, urllib.parse.urlencode(redacted), parsed.fragment))


if __name__ == "__main__":
    raise SystemExit(main())
