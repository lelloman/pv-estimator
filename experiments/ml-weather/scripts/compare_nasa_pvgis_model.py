#!/usr/bin/env python3
"""Compare NASA POWER, PVGIS, and the compressed climate-normal PV estimate."""

from __future__ import annotations

import argparse
import csv
import json
import math
import statistics
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

from compare_pvgis_climate_model import (  # noqa: E402
    DEFAULT_DATABASES,
    estimate_model_pv,
    fetch_pvgis,
    load_model,
    pct,
    read_locations,
    safe_name,
    solar_cosines,
)

NASA_API = "https://power.larc.nasa.gov/api/temporal/hourly/point"
NASA_PARAMETERS = [
    "ALLSKY_SFC_SW_DWN",
    "ALLSKY_SFC_SW_DNI",
    "ALLSKY_SFC_SW_DIFF",
    "T2M",
    "WS2M",
]
OUTPUT_FIELDS = [
    "location_id",
    "name",
    "region",
    "latitude",
    "longitude",
    "model_energy_kwh",
    "model_poa_kwh_m2",
    "model_ghi_kwh_m2",
    "nasa_energy_mean_kwh",
    "nasa_energy_std_kwh",
    "nasa_poa_mean_kwh_m2",
    "nasa_ghi_mean_kwh_m2",
    "model_minus_nasa_energy_pct",
    "model_minus_nasa_poa_pct",
    "pvgis_sarah3_energy_kwh",
    "nasa_minus_sarah3_energy_pct",
    "model_minus_sarah3_energy_pct",
    "pvgis_era5_energy_kwh",
    "nasa_minus_era5_energy_pct",
    "model_minus_era5_energy_pct",
]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--locations", type=Path, default=Path("experiments/ml-weather/config/pvgis_benchmark_locations.csv"))
    parser.add_argument("--checkpoint", type=Path, required=True)
    parser.add_argument("--out-csv", type=Path, default=Path("experiments/ml-weather/runs/nasa_pvgis_model_comparison_30.csv"))
    parser.add_argument("--out-json", type=Path, default=Path("experiments/ml-weather/runs/nasa_pvgis_model_comparison_30.summary.json"))
    parser.add_argument("--nasa-cache", type=Path, default=Path("experiments/ml-weather/runs/nasa_power_cache"))
    parser.add_argument("--pvgis-cache", type=Path, default=Path("experiments/ml-weather/runs/pvgis_cache"))
    parser.add_argument("--databases", default=",".join(DEFAULT_DATABASES))
    parser.add_argument("--start", default="20200101")
    parser.add_argument("--end", default="20241231")
    parser.add_argument("--peak-power-kwp", type=float, default=1.0)
    parser.add_argument("--loss-pct", type=float, default=14.0)
    parser.add_argument("--angle-deg", type=float, default=30.0)
    parser.add_argument("--aspect-deg", type=float, default=0.0, help="PVGIS convention: 0=south, 90=west, -90=east")
    parser.add_argument("--request-delay-seconds", type=float, default=0.75)
    parser.add_argument("--timeout-seconds", type=float, default=120.0)
    parser.add_argument("--retries", type=int, default=3)
    parser.add_argument("--limit", type=int, default=None)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    databases = [item.strip() for item in args.databases.split(",") if item.strip()]
    locations = read_locations(args.locations)
    if args.limit is not None:
        locations = locations[: args.limit]
    args.out_csv.parent.mkdir(parents=True, exist_ok=True)
    args.out_json.parent.mkdir(parents=True, exist_ok=True)
    args.nasa_cache.mkdir(parents=True, exist_ok=True)
    args.pvgis_cache.mkdir(parents=True, exist_ok=True)

    model, checkpoint = load_model(args.checkpoint)
    pvgis_args = SimpleNamespace(
        peak_power_kwp=args.peak_power_kwp,
        loss_pct=args.loss_pct,
        angle_deg=args.angle_deg,
        aspect_deg=args.aspect_deg,
        pvgis_cache=args.pvgis_cache,
        retries=args.retries,
        timeout_seconds=args.timeout_seconds,
    )

    rows: list[dict[str, Any]] = []
    for index, location in enumerate(locations, start=1):
        lat = float(location["latitude"])
        lon = float(location["longitude"])
        model_estimate = estimate_model_pv(
            model,
            checkpoint,
            lat,
            lon,
            args.angle_deg,
            args.aspect_deg,
            args.peak_power_kwp,
            args.loss_pct,
        )
        nasa = fetch_nasa(args, location["location_id"], lat, lon)
        nasa_estimate = estimate_nasa_pv(
            nasa,
            lat,
            lon,
            args.angle_deg,
            args.aspect_deg,
            args.peak_power_kwp,
            args.loss_pct,
        )
        pvgis_results: dict[str, dict[str, float]] = {}
        for database in databases:
            try:
                pvgis_results[database] = fetch_pvgis(pvgis_args, location["location_id"], lat, lon, database)
            except Exception as exc:
                print(f"[{index}/{len(locations)}] {location['location_id']} {database} failed: {exc}", flush=True)
            time.sleep(args.request_delay_seconds)

        row = build_row(location, model_estimate, nasa_estimate, pvgis_results)
        rows.append(row)
        print(
            f"[{index}/{len(locations)}] {location['location_id']} "
            f"NASA={row['nasa_energy_mean_kwh']:.2f} model={row['model_energy_kwh']:.2f} "
            f"ERA5={format_optional(row.get('pvgis_era5_energy_kwh'))} "
            f"SARAH3={format_optional(row.get('pvgis_sarah3_energy_kwh'))}",
            flush=True,
        )
        time.sleep(args.request_delay_seconds)

    write_csv(args.out_csv, rows)
    summary = summarize(rows, args, databases)
    args.out_json.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {args.out_csv}", flush=True)
    print(f"wrote {args.out_json}", flush=True)
    print_summary(summary)
    return 0


def fetch_nasa(args: argparse.Namespace, location_id: str, lat: float, lon: float) -> dict[str, Any]:
    cache_path = args.nasa_cache / f"{safe_name(location_id)}_{args.start}_{args.end}.json"
    if cache_path.exists():
        return json.loads(cache_path.read_text(encoding="utf-8"))
    params = {
        "parameters": ",".join(NASA_PARAMETERS),
        "community": "RE",
        "longitude": lon,
        "latitude": lat,
        "start": args.start,
        "end": args.end,
        "format": "JSON",
        "time-standard": "UTC",
    }
    url = f"{NASA_API}?{urllib.parse.urlencode(params)}"
    data = request_json(url, args.retries, args.timeout_seconds)
    cache_path.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return data


def request_json(url: str, retries: int, timeout_seconds: float) -> dict[str, Any]:
    last_error: Exception | None = None
    for attempt in range(retries + 1):
        try:
            request = urllib.request.Request(url, headers={"User-Agent": "pv-estimator-source-benchmark/0.1"})
            with urllib.request.urlopen(request, timeout=timeout_seconds) as response:
                return json.load(response)
        except urllib.error.HTTPError as exc:
            last_error = exc
            if exc.code in (429, 529) and attempt < retries:
                time.sleep(5.0 * (attempt + 1))
                continue
            raise
        except (TimeoutError, urllib.error.URLError) as exc:
            last_error = exc
            if attempt < retries:
                time.sleep(5.0 * (attempt + 1))
                continue
    raise RuntimeError(f"request failed after {retries + 1} attempts: {last_error}")


def estimate_nasa_pv(
    data: dict[str, Any],
    lat: float,
    lon: float,
    angle_deg: float,
    aspect_deg: float,
    peak_power_kwp: float,
    loss_pct: float,
) -> dict[str, Any]:
    params = data["properties"]["parameter"]
    ghi_values = params["ALLSKY_SFC_SW_DWN"]
    dni_values = params["ALLSKY_SFC_SW_DNI"]
    dhi_values = params["ALLSKY_SFC_SW_DIFF"]
    temp_values = params["T2M"]

    lat_rad = math.radians(lat)
    tilt = math.radians(angle_deg)
    surface_azimuth_from_north = math.radians((180.0 + aspect_deg) % 360.0)
    loss_factor = 1.0 - loss_pct / 100.0
    albedo = 0.2
    gamma = -0.0040
    noct_c = 45.0
    yearly: dict[int, dict[str, float]] = {}

    for key in sorted(ghi_values):
        ghi = clean_nasa_value(ghi_values.get(key))
        dni = clean_nasa_value(dni_values.get(key))
        dhi = clean_nasa_value(dhi_values.get(key))
        temp = clean_nasa_value(temp_values.get(key))
        if ghi is None or dni is None or dhi is None or temp is None:
            continue
        timestamp = datetime.strptime(key, "%Y%m%d%H").replace(tzinfo=timezone.utc)
        doy = float(timestamp.timetuple().tm_yday)
        utc_hour = float(timestamp.hour) + 0.5
        cosz, cosi = solar_cosines(lat_rad, lon, surface_azimuth_from_north, tilt, doy, utc_hour)
        beam = dni * cosi if cosz > 0.0 else 0.0
        diffuse = dhi * (1.0 + math.cos(tilt)) / 2.0
        ground = ghi * albedo * (1.0 - math.cos(tilt)) / 2.0
        poa = max(0.0, beam + diffuse + ground)
        cell_temp = temp + poa * (noct_c - 20.0) / 800.0
        temp_factor = max(0.0, 1.0 + gamma * (cell_temp - 25.0))
        energy = peak_power_kwp * (poa / 1000.0) * temp_factor * loss_factor
        bucket = yearly.setdefault(timestamp.year, {"energy_kwh": 0.0, "poa_kwh_m2": 0.0, "ghi_kwh_m2": 0.0, "hours": 0.0})
        bucket["energy_kwh"] += energy
        bucket["poa_kwh_m2"] += poa / 1000.0
        bucket["ghi_kwh_m2"] += ghi / 1000.0
        bucket["hours"] += 1.0

    complete_years = {year: values for year, values in yearly.items() if values["hours"] >= 8700.0}
    if not complete_years:
        raise RuntimeError("NASA response did not contain a complete enough year")
    energies = [values["energy_kwh"] for values in complete_years.values()]
    poas = [values["poa_kwh_m2"] for values in complete_years.values()]
    ghis = [values["ghi_kwh_m2"] for values in complete_years.values()]
    return {
        "energy_mean_kwh": statistics.fmean(energies),
        "energy_std_kwh": statistics.stdev(energies) if len(energies) > 1 else 0.0,
        "poa_mean_kwh_m2": statistics.fmean(poas),
        "ghi_mean_kwh_m2": statistics.fmean(ghis),
        "years": complete_years,
    }


def clean_nasa_value(value: Any) -> float | None:
    if value is None:
        return None
    value = float(value)
    if value <= -990.0:
        return None
    return max(value, 0.0) if value in (-0.0, 0.0) else value


def build_row(
    location: dict[str, str],
    model: dict[str, Any],
    nasa: dict[str, Any],
    pvgis: dict[str, dict[str, float]],
) -> dict[str, Any]:
    row: dict[str, Any] = {
        "location_id": location["location_id"],
        "name": location["name"],
        "region": location["region"],
        "latitude": float(location["latitude"]),
        "longitude": float(location["longitude"]),
        "model_energy_kwh": model["energy_kwh"],
        "model_poa_kwh_m2": model["poa_kwh_m2"],
        "model_ghi_kwh_m2": model["ghi_kwh_m2"],
        "nasa_energy_mean_kwh": nasa["energy_mean_kwh"],
        "nasa_energy_std_kwh": nasa["energy_std_kwh"],
        "nasa_poa_mean_kwh_m2": nasa["poa_mean_kwh_m2"],
        "nasa_ghi_mean_kwh_m2": nasa["ghi_mean_kwh_m2"],
        "model_minus_nasa_energy_pct": pct(model["energy_kwh"] - nasa["energy_mean_kwh"], nasa["energy_mean_kwh"]),
        "model_minus_nasa_poa_pct": pct(model["poa_kwh_m2"] - nasa["poa_mean_kwh_m2"], nasa["poa_mean_kwh_m2"]),
    }
    for database, prefix in [("PVGIS-SARAH3", "sarah3"), ("PVGIS-ERA5", "era5")]:
        result = pvgis.get(database)
        if result is None:
            continue
        row[f"pvgis_{prefix}_energy_kwh"] = result["energy_kwh"]
        row[f"nasa_minus_{prefix}_energy_pct"] = pct(nasa["energy_mean_kwh"] - result["energy_kwh"], result["energy_kwh"])
        row[f"model_minus_{prefix}_energy_pct"] = pct(model["energy_kwh"] - result["energy_kwh"], result["energy_kwh"])
    return row


def write_csv(path: Path, rows: list[dict[str, Any]]) -> None:
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=OUTPUT_FIELDS, extrasaction="ignore")
        writer.writeheader()
        for row in rows:
            writer.writerow({field: row.get(field, "") for field in OUTPUT_FIELDS})


def summarize(rows: list[dict[str, Any]], args: argparse.Namespace, databases: list[str]) -> dict[str, Any]:
    summary: dict[str, Any] = {
        "locations": len(rows),
        "nasa_api": NASA_API,
        "nasa_start": args.start,
        "nasa_end": args.end,
        "databases": databases,
        "checkpoint": str(args.checkpoint),
        "system": {
            "peak_power_kwp": args.peak_power_kwp,
            "loss_pct": args.loss_pct,
            "angle_deg": args.angle_deg,
            "aspect_deg": args.aspect_deg,
        },
    }
    add_metric(summary, "model_vs_nasa", rows, "model_minus_nasa_energy_pct")
    add_metric(summary, "model_vs_nasa_poa", rows, "model_minus_nasa_poa_pct")
    for key, field in [
        ("nasa_vs_sarah3", "nasa_minus_sarah3_energy_pct"),
        ("model_vs_sarah3", "model_minus_sarah3_energy_pct"),
        ("nasa_vs_era5", "nasa_minus_era5_energy_pct"),
        ("model_vs_era5", "model_minus_era5_energy_pct"),
    ]:
        add_metric(summary, key, rows, field)
    yearly_std = [float(row["nasa_energy_std_kwh"]) for row in rows if row.get("nasa_energy_std_kwh") != ""]
    if yearly_std:
        summary["nasa_year_to_year"] = {
            "mean_std_kwh": statistics.fmean(yearly_std),
            "median_std_kwh": statistics.median(yearly_std),
        }
    return summary


def add_metric(summary: dict[str, Any], key: str, rows: list[dict[str, Any]], field: str) -> None:
    values = [float(row[field]) for row in rows if field in row and row[field] != ""]
    if not values:
        return
    summary[key] = {
        "count": len(values),
        "mbe_pct": statistics.fmean(values),
        "mae_pct": statistics.fmean(abs(value) for value in values),
        "rmse_pct": math.sqrt(statistics.fmean(value * value for value in values)),
    }


def print_summary(summary: dict[str, Any]) -> None:
    for key in ["model_vs_nasa", "nasa_vs_sarah3", "model_vs_sarah3", "nasa_vs_era5", "model_vs_era5"]:
        if key not in summary:
            continue
        value = summary[key]
        print(
            f"{key}: n={value['count']} mbe={value['mbe_pct']:.2f}% "
            f"mae={value['mae_pct']:.2f}% rmse={value['rmse_pct']:.2f}%",
            flush=True,
        )


def format_optional(value: Any) -> str:
    if value is None or value == "":
        return "n/a"
    return f"{float(value):.2f}"


if __name__ == "__main__":
    raise SystemExit(main())
