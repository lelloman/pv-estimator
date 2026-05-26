#!/usr/bin/env python3
"""Normalize NASA POWER hourly JSON into a canonical CSV training table."""

from __future__ import annotations

import argparse
import csv
import gzip
import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

SOURCE_ID = "nasa_power_hourly"
SOURCE_RECORD_TYPE = "historical"
MISSING_SENTINELS = {-999, -999.0, -9999, -9999.0}

FIELD_MAP = {
    "ALLSKY_SFC_SW_DWN": "ghi_w_m2",
    "ALLSKY_SFC_SW_DNI": "dni_w_m2",
    "ALLSKY_SFC_SW_DIFF": "dhi_w_m2",
    "T2M": "ambient_temperature_c",
    "WS2M": "wind_speed_m_s",
}

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


def clean_value(value: Any) -> str:
    if value is None:
        return ""
    if isinstance(value, (int, float)) and value in MISSING_SENTINELS:
        return ""
    return str(value)


def timestamp_from_key(key: str) -> str:
    parsed = datetime.strptime(key, "%Y%m%d%H").replace(tzinfo=timezone.utc)
    return parsed.isoformat().replace("+00:00", "Z")


def location_id_from_path(path: Path) -> str:
    name = path.stem
    parts = name.rsplit("_", 2)
    if len(parts) == 3 and parts[1].isdigit() and parts[2].isdigit():
        return parts[0]
    return name


def normalize_file(path: Path, writer: csv.DictWriter[str]) -> dict[str, int]:
    data = json.loads(path.read_text(encoding="utf-8"))
    parameters = data["properties"]["parameter"]
    coordinates = data.get("geometry", {}).get("coordinates", [None, None, None])
    longitude = coordinates[0]
    latitude = coordinates[1]
    elevation = coordinates[2] if len(coordinates) > 2 else None
    location_id = location_id_from_path(path)

    keys: set[str] = set()
    for source_field in FIELD_MAP:
        keys.update(parameters.get(source_field, {}).keys())

    rows = 0
    missing_values = 0
    for key in sorted(keys):
        row = {
            "source_id": SOURCE_ID,
            "source_record_type": SOURCE_RECORD_TYPE,
            "location_id": location_id,
            "timestamp_utc": timestamp_from_key(key),
            "latitude": latitude,
            "longitude": longitude,
            "elevation_m": clean_value(elevation),
            "ghi_w_m2": "",
            "dni_w_m2": "",
            "dhi_w_m2": "",
            "ambient_temperature_c": "",
            "wind_speed_m_s": "",
            "wind_speed_height_m": "2",
            "quality_flags": "",
        }
        flags: list[str] = []
        for source_field, target_field in FIELD_MAP.items():
            raw_value = parameters.get(source_field, {}).get(key)
            cleaned = clean_value(raw_value)
            if cleaned == "":
                flags.append(f"missing:{target_field}")
                missing_values += 1
            row[target_field] = cleaned
        row["quality_flags"] = ";".join(flags)
        writer.writerow(row)
        rows += 1

    return {"rows": rows, "missing_values": missing_values}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--raw-dir", type=Path, default=Path("experiments/ml-weather/runs/pilot/raw/nasa_power_hourly"))
    parser.add_argument("--out", type=Path, default=Path("experiments/ml-weather/runs/pilot/normalized/nasa_power_hourly.csv.gz"))
    args = parser.parse_args()

    raw_files = sorted(path for path in args.raw_dir.glob("*.json") if path.name != "manifest.json")
    if not raw_files:
        raise SystemExit(f"no raw NASA POWER JSON files found in {args.raw_dir}")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    summary = {
        "source_id": SOURCE_ID,
        "created_at_utc": datetime.now(timezone.utc).isoformat(),
        "raw_dir": str(args.raw_dir),
        "output_path": str(args.out),
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
    print(f"rows={summary['rows']} missing_values={summary['missing_values']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
