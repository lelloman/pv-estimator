#!/usr/bin/env python3
"""Normalize direct NSRDB PSM3 CSV files into canonical hourly weather CSV."""

from __future__ import annotations

import argparse
import csv
import gzip
import json
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

SOURCE_ID = "nsrdb_psm3_hourly"
SOURCE_RECORD_TYPE = "historical"
CSV_FIELDS = [
    "source_id",
    "source_record_type",
    "location_id",
    "timestamp_utc",
    "latitude",
    "longitude",
    "elevation_m",
    "ghi_w_m2",
    "dni_w_m2",
    "dhi_w_m2",
    "ambient_temperature_c",
    "wind_speed_m_s",
    "wind_speed_height_m",
    "quality_flags",
]
FIELD_ALIASES = {
    "ghi_w_m2": ["GHI", "ghi"],
    "dni_w_m2": ["DNI", "dni"],
    "dhi_w_m2": ["DHI", "dhi"],
    "ambient_temperature_c": ["Temperature", "Air Temperature", "air_temperature"],
    "wind_speed_m_s": ["Wind Speed", "wind_speed"],
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--raw-dir", type=Path, default=Path("experiments/ml-weather/runs/nsrdb_psm3/raw"))
    parser.add_argument("--out", type=Path, default=Path("experiments/ml-weather/runs/nsrdb_psm3/normalized/nsrdb_psm3.csv.gz"))
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    raw_files = sorted(path for path in args.raw_dir.rglob("*.csv") if not path.name.endswith(".error.csv"))
    if not raw_files:
        raise SystemExit(f"no NSRDB CSV files found in {args.raw_dir}")
    args.out.parent.mkdir(parents=True, exist_ok=True)
    summary: dict[str, Any] = {
        "source_id": SOURCE_ID,
        "raw_dir": str(args.raw_dir),
        "output_path": str(args.out),
        "created_at_utc": datetime.now(timezone.utc).isoformat(),
        "files": [],
        "rows": 0,
        "missing_values": 0,
    }
    with gzip.open(args.out, "wt", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=CSV_FIELDS)
        writer.writeheader()
        for raw_file in raw_files:
            file_summary = normalize_file(raw_file, writer)
            summary["files"].append({"path": str(raw_file), **file_summary})
            summary["rows"] += file_summary["rows"]
            summary["missing_values"] += file_summary["missing_values"]
    summary_path = args.out.with_suffix(args.out.suffix + ".summary.json")
    summary_path.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {args.out}")
    print(f"wrote {summary_path}")
    print(f"files={len(raw_files)} rows={summary['rows']} missing_values={summary['missing_values']}")
    return 0


def normalize_file(path: Path, writer: csv.DictWriter[str]) -> dict[str, Any]:
    rows = list(csv.reader(path.open(newline="", encoding="utf-8-sig")))
    header_index = find_data_header(rows)
    if header_index is None:
        raise RuntimeError(f"could not find NSRDB data header in {path}")
    metadata = parse_metadata(rows[:header_index])
    data_header = rows[header_index]
    field_index = {name: index for index, name in enumerate(data_header)}
    location_id = location_id_from_path(path)
    latitude = metadata.get("Latitude") or metadata.get("lat") or ""
    longitude = metadata.get("Longitude") or metadata.get("lon") or ""
    elevation = metadata.get("Elevation") or metadata.get("elevation") or ""
    missing = 0
    out_rows = 0
    for values in rows[header_index + 1 :]:
        if not values or len(values) < len(data_header):
            continue
        record = {name: values[index] for name, index in field_index.items()}
        canonical = {field: get_first(record, aliases) for field, aliases in FIELD_ALIASES.items()}
        flags = ["source_dataset:NSRDB_PSM3"]
        for field, value in canonical.items():
            if value == "":
                flags.append(f"missing:{field}")
                missing += 1
        writer.writerow(
            {
                "source_id": SOURCE_ID,
                "source_record_type": SOURCE_RECORD_TYPE,
                "location_id": location_id,
                "timestamp_utc": parse_timestamp(record),
                "latitude": latitude,
                "longitude": longitude,
                "elevation_m": elevation,
                "ghi_w_m2": canonical["ghi_w_m2"],
                "dni_w_m2": canonical["dni_w_m2"],
                "dhi_w_m2": canonical["dhi_w_m2"],
                "ambient_temperature_c": canonical["ambient_temperature_c"],
                "wind_speed_m_s": canonical["wind_speed_m_s"],
                "wind_speed_height_m": "10" if canonical["wind_speed_m_s"] != "" else "",
                "quality_flags": ";".join(flags),
            }
        )
        out_rows += 1
    return {"location_id": location_id, "rows": out_rows, "missing_values": missing}


def find_data_header(rows: list[list[str]]) -> int | None:
    for index, row in enumerate(rows):
        normalized = {item.strip() for item in row}
        if {"Year", "Month", "Day", "Hour"}.issubset(normalized):
            return index
    return None


def parse_metadata(rows: list[list[str]]) -> dict[str, str]:
    if len(rows) >= 2 and len(rows[0]) == len(rows[1]):
        return {key.strip(): value.strip() for key, value in zip(rows[0], rows[1])}
    return {}


def get_first(record: dict[str, str], aliases: list[str]) -> str:
    for alias in aliases:
        if alias in record and record[alias] != "":
            return record[alias]
    return ""


def parse_timestamp(record: dict[str, str]) -> str:
    year = int(record["Year"])
    month = int(record["Month"])
    day = int(record["Day"])
    hour = int(record.get("Hour", "0") or 0)
    minute = int(record.get("Minute", "0") or 0)
    parsed = datetime(year, month, day, tzinfo=timezone.utc) + timedelta(hours=hour, minutes=minute)
    return parsed.isoformat().replace("+00:00", "Z")


def location_id_from_path(path: Path) -> str:
    stem = path.stem
    parts = stem.rsplit("_", 1)
    if len(parts) == 2 and parts[1].isdigit():
        return parts[0]
    return stem


if __name__ == "__main__":
    raise SystemExit(main())
