#!/usr/bin/env python3
"""Compare the climate-normal compressor PV estimate against PVGIS PVcalc."""

from __future__ import annotations

import argparse
import csv
import json
import math
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

import numpy as np
import torch

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

from train_climate_normals_torch import ClimateNormalMlp, encode_features  # noqa: E402

MONTH_DAYS = np.array([31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31], dtype=np.float64)
MID_MONTH_DOY = np.array([15, 46, 74, 105, 135, 166, 196, 227, 258, 288, 319, 349], dtype=np.float64)
PVGIS_API = "https://re.jrc.ec.europa.eu/api/v5_3/PVcalc"
DEFAULT_DATABASES = ["PVGIS-SARAH3", "PVGIS-ERA5"]
OUTPUT_FIELDS = [
    "location_id",
    "name",
    "region",
    "latitude",
    "longitude",
    "angle_deg",
    "aspect_deg",
    "peak_power_kwp",
    "loss_pct",
    "model_energy_kwh",
    "model_poa_kwh_m2",
    "model_ghi_kwh_m2",
    "pvgis_sarah3_energy_kwh",
    "pvgis_sarah3_poa_kwh_m2",
    "pvgis_sarah3_year_variability_kwh",
    "pvgis_sarah3_total_loss_pct",
    "pvgis_sarah3_energy_error_kwh",
    "pvgis_sarah3_energy_error_pct",
    "pvgis_sarah3_poa_error_pct",
    "pvgis_era5_energy_kwh",
    "pvgis_era5_poa_kwh_m2",
    "pvgis_era5_year_variability_kwh",
    "pvgis_era5_total_loss_pct",
    "pvgis_era5_energy_error_kwh",
    "pvgis_era5_energy_error_pct",
    "pvgis_era5_poa_error_pct",
    "pvgis_energy_spread_kwh",
    "pvgis_energy_spread_pct_of_midpoint",
    "model_inside_pvgis_energy_range",
]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--locations", type=Path, default=Path("experiments/ml-weather/config/pvgis_benchmark_locations.csv"))
    parser.add_argument("--checkpoint", type=Path, required=True)
    parser.add_argument("--out-csv", type=Path, default=Path("experiments/ml-weather/runs/pvgis_comparison_30.csv"))
    parser.add_argument("--out-json", type=Path, default=Path("experiments/ml-weather/runs/pvgis_comparison_30.summary.json"))
    parser.add_argument("--pvgis-cache", type=Path, default=Path("experiments/ml-weather/runs/pvgis_cache"))
    parser.add_argument("--databases", default=",".join(DEFAULT_DATABASES))
    parser.add_argument("--peak-power-kwp", type=float, default=1.0)
    parser.add_argument("--loss-pct", type=float, default=14.0)
    parser.add_argument("--angle-deg", type=float, default=30.0)
    parser.add_argument("--aspect-deg", type=float, default=0.0, help="PVGIS convention: 0=south, 90=west, -90=east")
    parser.add_argument("--request-delay-seconds", type=float, default=0.75)
    parser.add_argument("--timeout-seconds", type=float, default=60.0)
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
    args.pvgis_cache.mkdir(parents=True, exist_ok=True)

    model, checkpoint = load_model(args.checkpoint)
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
        pvgis_results: dict[str, dict[str, float]] = {}
        for database in databases:
            try:
                pvgis_results[database] = fetch_pvgis(
                    args,
                    location["location_id"],
                    lat,
                    lon,
                    database,
                )
            except Exception as exc:  # PVGIS coverage differs by database and location.
                print(f"[{index}/{len(locations)}] {location['location_id']} {database} failed: {exc}", flush=True)
            time.sleep(args.request_delay_seconds)
        row = build_row(args, location, model_estimate, pvgis_results)
        rows.append(row)
        print(
            f"[{index}/{len(locations)}] {location['location_id']} "
            f"model={row['model_energy_kwh']:.2f} "
            f"SARAH3={format_optional(row.get('pvgis_sarah3_energy_kwh'))} "
            f"ERA5={format_optional(row.get('pvgis_era5_energy_kwh'))}",
            flush=True,
        )

    write_csv(args.out_csv, rows)
    summary = summarize(rows, args, databases)
    args.out_json.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {args.out_csv}", flush=True)
    print(f"wrote {args.out_json}", flush=True)
    print_summary(summary)
    return 0


def read_locations(path: Path) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def load_model(path: Path) -> tuple[ClimateNormalMlp, dict[str, Any]]:
    checkpoint = torch.load(path, map_location="cpu", weights_only=False)
    model = ClimateNormalMlp(
        int(checkpoint["input_features"]),
        int(checkpoint["hidden_width"]),
        int(checkpoint["residual_blocks"]),
        float(checkpoint["residual_scale"]),
    )
    model.load_state_dict(checkpoint["model_state_dict"])
    model.eval()
    return model, checkpoint


def estimate_model_pv(
    model: ClimateNormalMlp,
    checkpoint: dict[str, Any],
    lat: float,
    lon: float,
    angle_deg: float,
    aspect_deg: float,
    peak_power_kwp: float,
    loss_pct: float,
) -> dict[str, Any]:
    rows = [(month, hour) for month in range(12) for hour in range(24)]
    months = np.array([row[0] for row in rows], dtype=np.int64)
    hours = np.array([row[1] for row in rows], dtype=np.int64)
    x = encode_features(
        np.full(len(rows), lat, dtype=np.float32),
        np.full(len(rows), lon, dtype=np.float32),
        months,
        hours,
    )
    with torch.no_grad():
        y = model(torch.from_numpy(x)).numpy()
    y = y * checkpoint["target_std"].reshape(1, 10) + checkpoint["target_mean"].reshape(1, 10)
    y[:, 0:5] = np.maximum(y[:, 0:5], 0.0)

    lat_rad = math.radians(lat)
    tilt = math.radians(angle_deg)
    surface_azimuth_from_north = math.radians((180.0 + aspect_deg) % 360.0)
    loss_factor = 1.0 - loss_pct / 100.0
    albedo = 0.2
    gamma = -0.0040
    noct_c = 45.0
    monthly: list[dict[str, float]] = []
    annual_poa = 0.0
    annual_energy = 0.0
    annual_ghi = 0.0

    for month in range(12):
        poa_day = 0.0
        energy_day = 0.0
        ghi_day = 0.0
        for hour in range(24):
            idx = month * 24 + hour
            ghi, dni, dhi, temp, _wind = y[idx, 0:5].astype(float)
            cosz, cosi = solar_cosines(lat_rad, lon, surface_azimuth_from_north, tilt, float(MID_MONTH_DOY[month]), hour + 0.5)
            beam = dni * cosi if cosz > 0.0 else 0.0
            diffuse = dhi * (1.0 + math.cos(tilt)) / 2.0
            ground = ghi * albedo * (1.0 - math.cos(tilt)) / 2.0
            poa = max(0.0, beam + diffuse + ground)
            cell_temp = temp + poa * (noct_c - 20.0) / 800.0
            temp_factor = max(0.0, 1.0 + gamma * (cell_temp - 25.0))
            energy = peak_power_kwp * (poa / 1000.0) * temp_factor * loss_factor
            poa_day += poa
            energy_day += energy
            ghi_day += ghi
        month_poa = poa_day * MONTH_DAYS[month] / 1000.0
        month_energy = energy_day * MONTH_DAYS[month]
        month_ghi = ghi_day * MONTH_DAYS[month] / 1000.0
        monthly.append({"month": month + 1, "poa_kwh_m2": month_poa, "energy_kwh": month_energy, "ghi_kwh_m2": month_ghi})
        annual_poa += month_poa
        annual_energy += month_energy
        annual_ghi += month_ghi

    return {
        "energy_kwh": annual_energy,
        "poa_kwh_m2": annual_poa,
        "ghi_kwh_m2": annual_ghi,
        "monthly": monthly,
    }


def solar_cosines(
    lat_rad: float,
    lon: float,
    surface_azimuth_from_north: float,
    tilt: float,
    doy: float,
    utc_hour: float,
) -> tuple[float, float]:
    gamma_day = 2.0 * math.pi / 365.0 * (doy - 1.0 + (utc_hour - 12.0) / 24.0)
    eqtime = 229.18 * (
        0.000075
        + 0.001868 * math.cos(gamma_day)
        - 0.032077 * math.sin(gamma_day)
        - 0.014615 * math.cos(2 * gamma_day)
        - 0.040849 * math.sin(2 * gamma_day)
    )
    decl = (
        0.006918
        - 0.399912 * math.cos(gamma_day)
        + 0.070257 * math.sin(gamma_day)
        - 0.006758 * math.cos(2 * gamma_day)
        + 0.000907 * math.sin(2 * gamma_day)
        - 0.002697 * math.cos(3 * gamma_day)
        + 0.00148 * math.sin(3 * gamma_day)
    )
    true_solar_time = (utc_hour * 60.0 + eqtime + 4.0 * lon) % 1440.0
    hour_angle = math.radians(true_solar_time / 4.0 - 180.0)
    cosz = math.sin(lat_rad) * math.sin(decl) + math.cos(lat_rad) * math.cos(decl) * math.cos(hour_angle)
    cosz = max(cosz, 0.0)
    if cosz <= 0.0:
        return 0.0, 0.0
    solar_altitude = math.asin(max(-1.0, min(1.0, cosz)))
    cos_altitude = max(math.cos(solar_altitude), 1e-9)
    sin_azimuth = -math.sin(hour_angle) * math.cos(decl) / cos_altitude
    cos_azimuth = (math.sin(decl) - math.sin(solar_altitude) * math.sin(lat_rad)) / (cos_altitude * math.cos(lat_rad))
    azimuth = math.atan2(sin_azimuth, cos_azimuth)
    if azimuth < 0.0:
        azimuth += 2.0 * math.pi
    cos_incidence = (
        math.cos(solar_altitude) * math.sin(tilt) * math.cos(azimuth - surface_azimuth_from_north)
        + math.sin(solar_altitude) * math.cos(tilt)
    )
    return cosz, max(cos_incidence, 0.0)


def fetch_pvgis(args: argparse.Namespace, location_id: str, lat: float, lon: float, database: str) -> dict[str, float]:
    cache_path = args.pvgis_cache / f"{safe_name(location_id)}_{safe_name(database)}.json"
    if cache_path.exists():
        data = json.loads(cache_path.read_text(encoding="utf-8"))
    else:
        params = {
            "lat": lat,
            "lon": lon,
            "peakpower": args.peak_power_kwp,
            "loss": args.loss_pct,
            "angle": args.angle_deg,
            "aspect": args.aspect_deg,
            "pvtechchoice": "crystSi",
            "raddatabase": database,
            "outputformat": "json",
        }
        url = f"{PVGIS_API}?{urllib.parse.urlencode(params)}"
        data = request_json(url, args.retries, args.timeout_seconds)
        cache_path.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    fixed = data["outputs"]["totals"]["fixed"]
    monthly = data["outputs"]["monthly"]["fixed"]
    return {
        "energy_kwh": float(fixed["E_y"]),
        "poa_kwh_m2": float(fixed["H(i)_y"]),
        "year_variability_kwh": float(fixed["SD_y"]),
        "total_loss_pct": float(fixed["l_total"]),
        "monthly_energy_kwh": [float(row["E_m"]) for row in monthly],
        "monthly_poa_kwh_m2": [float(row["H(i)_m"]) for row in monthly],
    }


def request_json(url: str, retries: int, timeout_seconds: float) -> dict[str, Any]:
    last_error: Exception | None = None
    for attempt in range(retries + 1):
        try:
            request = urllib.request.Request(url, headers={"User-Agent": "pv-estimator-pvgis-benchmark/0.1"})
            with urllib.request.urlopen(request, timeout=timeout_seconds) as response:
                return json.load(response)
        except urllib.error.HTTPError as exc:
            last_error = exc
            if exc.code in (429, 529) and attempt < retries:
                time.sleep(2.0 * (attempt + 1))
                continue
            raise
        except (TimeoutError, urllib.error.URLError) as exc:
            last_error = exc
            if attempt < retries:
                time.sleep(2.0 * (attempt + 1))
                continue
    raise RuntimeError(f"request failed after {retries + 1} attempts: {last_error}")


def build_row(
    args: argparse.Namespace,
    location: dict[str, str],
    model_estimate: dict[str, Any],
    pvgis_results: dict[str, dict[str, float]],
) -> dict[str, Any]:
    row: dict[str, Any] = {
        "location_id": location["location_id"],
        "name": location["name"],
        "region": location["region"],
        "latitude": float(location["latitude"]),
        "longitude": float(location["longitude"]),
        "angle_deg": args.angle_deg,
        "aspect_deg": args.aspect_deg,
        "peak_power_kwp": args.peak_power_kwp,
        "loss_pct": args.loss_pct,
        "model_energy_kwh": model_estimate["energy_kwh"],
        "model_poa_kwh_m2": model_estimate["poa_kwh_m2"],
        "model_ghi_kwh_m2": model_estimate["ghi_kwh_m2"],
    }
    for database, prefix in [("PVGIS-SARAH3", "pvgis_sarah3"), ("PVGIS-ERA5", "pvgis_era5")]:
        result = pvgis_results.get(database)
        if result is None:
            continue
        row[f"{prefix}_energy_kwh"] = result["energy_kwh"]
        row[f"{prefix}_poa_kwh_m2"] = result["poa_kwh_m2"]
        row[f"{prefix}_year_variability_kwh"] = result["year_variability_kwh"]
        row[f"{prefix}_total_loss_pct"] = result["total_loss_pct"]
        row[f"{prefix}_energy_error_kwh"] = model_estimate["energy_kwh"] - result["energy_kwh"]
        row[f"{prefix}_energy_error_pct"] = pct(model_estimate["energy_kwh"] - result["energy_kwh"], result["energy_kwh"])
        row[f"{prefix}_poa_error_pct"] = pct(model_estimate["poa_kwh_m2"] - result["poa_kwh_m2"], result["poa_kwh_m2"])
    if "pvgis_sarah3_energy_kwh" in row and "pvgis_era5_energy_kwh" in row:
        lo = min(row["pvgis_sarah3_energy_kwh"], row["pvgis_era5_energy_kwh"])
        hi = max(row["pvgis_sarah3_energy_kwh"], row["pvgis_era5_energy_kwh"])
        midpoint = (lo + hi) / 2.0
        row["pvgis_energy_spread_kwh"] = hi - lo
        row["pvgis_energy_spread_pct_of_midpoint"] = pct(hi - lo, midpoint)
        row["model_inside_pvgis_energy_range"] = lo <= row["model_energy_kwh"] <= hi
    return row


def pct(delta: float, denominator: float) -> float:
    if denominator == 0.0:
        return float("nan")
    return 100.0 * delta / denominator


def write_csv(path: Path, rows: list[dict[str, Any]]) -> None:
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=OUTPUT_FIELDS, extrasaction="ignore")
        writer.writeheader()
        for row in rows:
            writer.writerow({field: row.get(field, "") for field in OUTPUT_FIELDS})


def summarize(rows: list[dict[str, Any]], args: argparse.Namespace, databases: list[str]) -> dict[str, Any]:
    summary: dict[str, Any] = {
        "locations": len(rows),
        "databases": databases,
        "checkpoint": str(args.checkpoint),
        "pvgis_api": PVGIS_API,
        "system": {
            "peak_power_kwp": args.peak_power_kwp,
            "loss_pct": args.loss_pct,
            "angle_deg": args.angle_deg,
            "aspect_deg": args.aspect_deg,
            "pvtechchoice": "crystSi",
        },
    }
    for prefix in ["pvgis_sarah3", "pvgis_era5"]:
        errors = np.array([row[f"{prefix}_energy_error_pct"] for row in rows if f"{prefix}_energy_error_pct" in row], dtype=np.float64)
        poa_errors = np.array([row[f"{prefix}_poa_error_pct"] for row in rows if f"{prefix}_poa_error_pct" in row], dtype=np.float64)
        if len(errors) == 0:
            continue
        summary[prefix] = {
            "count": int(len(errors)),
            "energy_mbe_pct": float(np.mean(errors)),
            "energy_mae_pct": float(np.mean(np.abs(errors))),
            "energy_rmse_pct": float(np.sqrt(np.mean(errors * errors))),
            "poa_mbe_pct": float(np.mean(poa_errors)),
            "poa_mae_pct": float(np.mean(np.abs(poa_errors))),
            "poa_rmse_pct": float(np.sqrt(np.mean(poa_errors * poa_errors))),
        }
    spreads = np.array([row["pvgis_energy_spread_pct_of_midpoint"] for row in rows if "pvgis_energy_spread_pct_of_midpoint" in row], dtype=np.float64)
    inside = [bool(row["model_inside_pvgis_energy_range"]) for row in rows if "model_inside_pvgis_energy_range" in row]
    if len(spreads) > 0:
        summary["source_spread"] = {
            "count": int(len(spreads)),
            "mean_spread_pct_of_midpoint": float(np.mean(spreads)),
            "median_spread_pct_of_midpoint": float(np.median(spreads)),
            "model_inside_range_count": int(sum(inside)),
            "model_inside_range_fraction": float(sum(inside) / len(inside)),
        }
    return summary


def print_summary(summary: dict[str, Any]) -> None:
    for key in ["pvgis_sarah3", "pvgis_era5"]:
        if key not in summary:
            continue
        value = summary[key]
        print(
            f"{key}: n={value['count']} energy_mbe={value['energy_mbe_pct']:.2f}% "
            f"energy_mae={value['energy_mae_pct']:.2f}% poa_mbe={value['poa_mbe_pct']:.2f}% "
            f"poa_mae={value['poa_mae_pct']:.2f}%",
            flush=True,
        )
    if "source_spread" in summary:
        value = summary["source_spread"]
        print(
            f"source_spread: mean={value['mean_spread_pct_of_midpoint']:.2f}% "
            f"median={value['median_spread_pct_of_midpoint']:.2f}% "
            f"inside={value['model_inside_range_count']}/{value['count']}",
            flush=True,
        )


def format_optional(value: Any) -> str:
    if value is None or value == "":
        return "n/a"
    return f"{float(value):.2f}"


def safe_name(value: str) -> str:
    return "".join(char.lower() if char.isalnum() else "_" for char in value).strip("_")


if __name__ == "__main__":
    raise SystemExit(main())
