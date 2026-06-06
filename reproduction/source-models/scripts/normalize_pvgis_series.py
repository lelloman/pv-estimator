#!/usr/bin/env python3
"""Normalize PVGIS seriescalc JSON into the canonical hourly weather CSV."""

from __future__ import annotations

import argparse
import csv
import gzip
import json
import math
import re
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

SOURCE_RECORD_TYPE = "historical"
MISSING_SENTINELS = {-999, -999.0, -9999, -9999.0}
SOURCE_ID_BY_DATABASE = {
    "PVGIS-ERA5": "pvgis_era5_hourly",
    "PVGIS-SARAH3": "pvgis_sarah3_hourly",
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
TIME_RE = re.compile(r"^(\d{4})(\d{2})(\d{2}):(\d{2})(\d{2})$")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--raw-dir", type=Path, default=Path("reproduction/source-models/runs/pvgis_series/raw"))
    parser.add_argument("--out", type=Path, default=Path("reproduction/source-models/runs/pvgis_series/normalized/pvgis_series.csv.gz"))
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    raw_files = sorted(path for path in args.raw_dir.rglob("*.json") if path.name != "manifest.json" and not path.name.endswith(".error.json"))
    if not raw_files:
        raise SystemExit(f"no PVGIS JSON files found in {args.raw_dir}")
    args.out.parent.mkdir(parents=True, exist_ok=True)
    summary: dict[str, Any] = {
        "source_family": "pvgis_seriescalc",
        "raw_dir": str(args.raw_dir),
        "output_path": str(args.out),
        "files": [],
        "rows": 0,
        "missing_values": 0,
        "created_at_utc": datetime.now(timezone.utc).isoformat(),
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
    data = json.loads(path.read_text(encoding="utf-8"))
    hourly = data.get("outputs", {}).get("hourly", [])
    if not isinstance(hourly, list):
        raise RuntimeError(f"missing outputs.hourly list in {path}")
    location = data.get("inputs", {}).get("location", {})
    meteo = data.get("inputs", {}).get("meteo_data", {})
    latitude = location.get("latitude")
    longitude = location.get("longitude")
    elevation = location.get("elevation")
    database = meteo.get("radiation_db") or database_from_path(path)
    source_id = SOURCE_ID_BY_DATABASE.get(str(database), f"pvgis_{str(database).lower().replace('-', '_')}_hourly")
    location_id = location_id_from_path(path)

    rows = 0
    missing_values = 0
    for item in hourly:
        flags = [f"radiation_db:{database}", "derived_dni_from_horizontal_beam"]
        ghi = first_clean(item, ["G(i)", "G(h)", "G"])
        beam_horizontal = first_clean(item, ["Gb(i)", "Gb(h)", "Gb"])
        dhi = first_clean(item, ["Gd(i)", "Gd(h)", "Gd"])
        temp = first_clean(item, ["T2m", "T2M"])
        wind = first_clean(item, ["WS10m", "WS10M", "WS2m", "WS2M"])
        altitude_deg = first_clean(item, ["H_sun"])

        if ghi is None and beam_horizontal is not None and dhi is not None:
            ghi = beam_horizontal + dhi
            flags.append("derived_ghi_from_beam_plus_diffuse")
        dni = derive_dni(beam_horizontal, altitude_deg, flags)

        values = {
            "ghi_w_m2": ghi,
            "dni_w_m2": dni,
            "dhi_w_m2": dhi,
            "ambient_temperature_c": temp,
            "wind_speed_m_s": wind,
        }
        for field, value in values.items():
            if value is None:
                flags.append(f"missing:{field}")
                missing_values += 1

        row = {
            "source_id": source_id,
            "source_record_type": SOURCE_RECORD_TYPE,
            "location_id": location_id,
            "timestamp_utc": parse_pvgis_timestamp(str(item["time"])),
            "latitude": clean_str(latitude),
            "longitude": clean_str(longitude),
            "elevation_m": clean_str(elevation),
            "ghi_w_m2": clean_str(values["ghi_w_m2"]),
            "dni_w_m2": clean_str(values["dni_w_m2"]),
            "dhi_w_m2": clean_str(values["dhi_w_m2"]),
            "ambient_temperature_c": clean_str(values["ambient_temperature_c"]),
            "wind_speed_m_s": clean_str(values["wind_speed_m_s"]),
            "wind_speed_height_m": "10" if wind is not None else "",
            "quality_flags": ";".join(flags),
        }
        writer.writerow(row)
        rows += 1
    return {"source_id": source_id, "database": database, "location_id": location_id, "rows": rows, "missing_values": missing_values}


def first_clean(item: dict[str, Any], keys: list[str]) -> float | None:
    for key in keys:
        if key in item:
            return clean_number(item[key])
    return None


def clean_number(value: Any) -> float | None:
    if value is None or value == "":
        return None
    try:
        number = float(value)
    except (TypeError, ValueError):
        return None
    if number in MISSING_SENTINELS or number <= -990.0:
        return None
    return number


def derive_dni(beam_horizontal: float | None, altitude_deg: float | None, flags: list[str]) -> float | None:
    if beam_horizontal is None:
        return None
    if altitude_deg is None:
        flags.append("missing:H_sun_for_dni")
        return None
    if altitude_deg <= 0.0:
        return 0.0 if abs(beam_horizontal) < 1e-6 else None
    sin_altitude = math.sin(math.radians(altitude_deg))
    if sin_altitude <= 1e-6:
        return None
    return max(0.0, beam_horizontal / sin_altitude)


def parse_pvgis_timestamp(value: str) -> str:
    match = TIME_RE.match(value)
    if match is None:
        raise ValueError(f"unsupported PVGIS timestamp: {value}")
    year, month, day, hour, minute = (int(part) for part in match.groups())
    parsed = datetime(year, month, day, hour, minute, tzinfo=timezone.utc)
    return parsed.isoformat().replace("+00:00", "Z")


def location_id_from_path(path: Path) -> str:
    stem = path.stem
    parts = stem.rsplit("_", 2)
    if len(parts) == 3 and parts[1].isdigit() and parts[2].isdigit():
        return parts[0]
    return stem


def database_from_path(path: Path) -> str:
    parent = path.parent.name.upper().replace("_", "-")
    if parent.startswith("PVGIS-"):
        return parent
    return parent


def clean_str(value: Any) -> str:
    if value is None:
        return ""
    if isinstance(value, float) and (math.isnan(value) or math.isinf(value)):
        return ""
    return str(value)


if __name__ == "__main__":
    raise SystemExit(main())
