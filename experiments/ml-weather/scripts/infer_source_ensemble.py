#!/usr/bin/env python3
"""Run the trained source climate-normal models as a small ensemble."""

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
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import numpy as np
import torch

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

from train_climate_normals_stream_torch import ClimateNormalMlp, encode_features  # noqa: E402

MONTH_DAYS = np.array([31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31], dtype=np.float64)
MID_MONTH_DOY = np.array([15, 46, 74, 105, 135, 166, 196, 227, 258, 288, 319, 349], dtype=np.float64)
PVGIS_API = "https://re.jrc.ec.europa.eu/api/v5_3/PVcalc"
DEFAULT_DATABASES = ["PVGIS-SARAH3", "PVGIS-ERA5"]
TARGET_NAMES = [
    "ghi_mean_w_m2",
    "dni_mean_w_m2",
    "dhi_mean_w_m2",
    "temp_mean_c",
    "wind_mean_m_s",
    "ghi_std_w_m2",
    "dni_std_w_m2",
    "dhi_std_w_m2",
    "temp_std_c",
    "wind_std_m_s",
]
OUTPUT_FIELDS = [
    "location_id",
    "name",
    "region",
    "latitude",
    "longitude",
    "applicable_sources",
    "source_count",
    "ensemble_energy_kwh",
    "ensemble_energy_low_kwh",
    "ensemble_energy_high_kwh",
    "ensemble_energy_half_spread_kwh",
    "ensemble_energy_spread_pct",
    "ensemble_poa_kwh_m2",
    "ensemble_poa_low_kwh_m2",
    "ensemble_poa_high_kwh_m2",
    "ensemble_ghi_kwh_m2",
    "nasa_power_energy_kwh",
    "nasa_power_poa_kwh_m2",
    "nasa_power_ghi_kwh_m2",
    "pvgis_era5_energy_kwh",
    "pvgis_era5_poa_kwh_m2",
    "pvgis_era5_ghi_kwh_m2",
    "pvgis_sarah3_energy_kwh",
    "pvgis_sarah3_poa_kwh_m2",
    "pvgis_sarah3_ghi_kwh_m2",
    "pvgis_sarah3_applicable",
    "pvgis_era5_reference_energy_kwh",
    "pvgis_era5_reference_poa_kwh_m2",
    "pvgis_era5_reference_year_variability_kwh",
    "pvgis_era5_reference_energy_error_pct",
    "pvgis_sarah3_reference_energy_kwh",
    "pvgis_sarah3_reference_poa_kwh_m2",
    "pvgis_sarah3_reference_year_variability_kwh",
    "pvgis_sarah3_reference_energy_error_pct",
]


@dataclass(frozen=True)
class SourceSpec:
    source_id: str
    label: str
    checkpoint: Path
    coverage: str


@dataclass
class LoadedSource:
    spec: SourceSpec
    model: ClimateNormalMlp
    checkpoint: dict[str, Any]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--locations", type=Path, default=Path("experiments/ml-weather/config/pvgis_benchmark_locations.csv"))
    parser.add_argument("--lat", type=float, default=None, help="single-location latitude in decimal degrees")
    parser.add_argument("--lon", type=float, default=None, help="single-location longitude in decimal degrees")
    parser.add_argument("--location-id", default="custom", help="single-location id used with --lat/--lon")
    parser.add_argument("--name", default="Custom location", help="single-location display name used with --lat/--lon")
    parser.add_argument("--region", default="", help="single-location region label used with --lat/--lon")
    parser.add_argument("--out-csv", type=Path, default=Path("experiments/ml-weather/runs/source_ensemble_predictions.csv"))
    parser.add_argument("--out-json", type=Path, default=Path("experiments/ml-weather/runs/source_ensemble_predictions.summary.json"))
    parser.add_argument("--out-estimate-json", type=Path, default=None, help="write Rust-compatible annual/monthly estimate JSON")
    parser.add_argument("--nasa-checkpoint", type=Path, required=True)
    parser.add_argument("--era5-checkpoint", type=Path, required=True)
    parser.add_argument("--sarah3-checkpoint", type=Path, required=True)
    parser.add_argument("--sarah3-mask", type=Path, required=True)
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("--peak-power-kwp", type=float, default=1.0)
    parser.add_argument("--loss-pct", type=float, default=14.0)
    parser.add_argument("--angle-deg", type=float, default=30.0)
    parser.add_argument("--aspect-deg", type=float, default=0.0, help="PVGIS convention: 0=south, 90=west, -90=east")
    parser.add_argument("--fetch-pvgis", action="store_true")
    parser.add_argument("--pvgis-cache", type=Path, default=Path("experiments/ml-weather/runs/source_ensemble_pvgis_cache"))
    parser.add_argument("--databases", default=",".join(DEFAULT_DATABASES))
    parser.add_argument("--request-delay-seconds", type=float, default=0.75)
    parser.add_argument("--timeout-seconds", type=float, default=60.0)
    parser.add_argument("--retries", type=int, default=3)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    locations = load_requested_locations(args)
    if args.limit is not None:
        locations = locations[: args.limit]
    args.out_csv.parent.mkdir(parents=True, exist_ok=True)
    args.out_json.parent.mkdir(parents=True, exist_ok=True)
    if args.out_estimate_json is not None:
        args.out_estimate_json.parent.mkdir(parents=True, exist_ok=True)
    args.pvgis_cache.mkdir(parents=True, exist_ok=True)
    sarah3_mask = CoverageMask.load(args.sarah3_mask)

    sources = [
        load_source(SourceSpec("nasa_power", "NASA POWER", args.nasa_checkpoint, "global")),
        load_source(SourceSpec("pvgis_era5", "PVGIS-ERA5", args.era5_checkpoint, "global")),
        load_source(SourceSpec("pvgis_sarah3", "PVGIS-SARAH3", args.sarah3_checkpoint, "sarah3_mask")),
    ]
    databases = [item.strip() for item in args.databases.split(",") if item.strip()]
    rows: list[dict[str, Any]] = []
    estimate_documents: list[dict[str, Any]] = []
    for index, location in enumerate(locations, start=1):
        lat = float(location["latitude"])
        lon = float(location["longitude"])
        predictions: dict[str, dict[str, Any]] = {}
        for source in sources:
            if not source_applies(source.spec, lat, lon, sarah3_mask):
                continue
            climate = predict_month_hour(source, lat, lon)
            predictions[source.spec.source_id] = estimate_pv_from_climate(
                climate,
                lat,
                lon,
                args.angle_deg,
                args.aspect_deg,
                args.peak_power_kwp,
                args.loss_pct,
            )
        references: dict[str, dict[str, float]] = {}
        if args.fetch_pvgis:
            for database in databases:
                try:
                    references[database] = fetch_pvgis(args, location["location_id"], lat, lon, database)
                except Exception as exc:
                    print(f"[{index}/{len(locations)}] {location['location_id']} {database} failed: {exc}", flush=True)
                time.sleep(args.request_delay_seconds)
        sarah3_applicable = sarah3_mask.contains(lat, lon)
        row = build_row(args, location, predictions, references, sarah3_applicable)
        rows.append(row)
        estimate_documents.append(build_estimate_document(args, location, predictions, references, sarah3_applicable))
        print(
            f"[{index}/{len(locations)}] {location['location_id']} "
            f"sources={row['applicable_sources']} ensemble={row['ensemble_energy_kwh']:.2f}kWh "
            f"range={row['ensemble_energy_low_kwh']:.2f}..{row['ensemble_energy_high_kwh']:.2f}",
            flush=True,
        )

    write_csv(args.out_csv, rows)
    summary = summarize(args, rows, sources)
    args.out_json.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    if args.out_estimate_json is not None:
        payload: dict[str, Any]
        if len(estimate_documents) == 1:
            payload = estimate_documents[0]
        else:
            payload = {"schema_version": 1, "estimates": estimate_documents}
        args.out_estimate_json.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {args.out_csv}", flush=True)
    print(f"wrote {args.out_json}", flush=True)
    if args.out_estimate_json is not None:
        print(f"wrote {args.out_estimate_json}", flush=True)
    print_summary(summary)
    return 0


def load_requested_locations(args: argparse.Namespace) -> list[dict[str, str]]:
    if args.lat is None and args.lon is None:
        return read_locations(args.locations)
    if args.lat is None or args.lon is None:
        raise SystemExit("--lat and --lon must be provided together")
    return [
        {
            "location_id": args.location_id,
            "name": args.name,
            "region": args.region,
            "latitude": str(args.lat),
            "longitude": str(args.lon),
        }
    ]


def read_locations(path: Path) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def load_source(spec: SourceSpec) -> LoadedSource:
    checkpoint = torch.load(spec.checkpoint.expanduser(), map_location="cpu", weights_only=False)
    model = ClimateNormalMlp(
        int(checkpoint["input_features"]),
        int(checkpoint["hidden_width"]),
        int(checkpoint["residual_blocks"]),
        float(checkpoint["residual_scale"]),
    )
    model.load_state_dict(checkpoint["model_state_dict"])
    model.eval()
    print(
        f"loaded {spec.source_id} checkpoint={spec.checkpoint} "
        f"epoch={checkpoint.get('epoch')} locations={checkpoint.get('location_count')}",
        flush=True,
    )
    return LoadedSource(spec, model, checkpoint)


def source_applies(spec: SourceSpec, lat: float, lon: float, sarah3_mask: "CoverageMask") -> bool:
    if spec.coverage == "global":
        return True
    if spec.coverage == "sarah3_mask":
        return sarah3_mask.contains(lat, lon)
    raise ValueError(f"unsupported coverage rule: {spec.coverage}")


def predict_month_hour(source: LoadedSource, lat: float, lon: float) -> np.ndarray:
    temporal_bins = int(source.checkpoint.get("temporal_bins", 288))
    if temporal_bins != 288:
        raise RuntimeError(f"{source.spec.source_id} expected month-hour model, got temporal_bins={temporal_bins}")
    temporal = np.arange(288, dtype=np.int64)
    x = encode_features(
        np.full(len(temporal), lat, dtype=np.float32),
        np.full(len(temporal), lon, dtype=np.float32),
        temporal,
        temporal_bins,
    )
    with torch.no_grad():
        y = source.model(torch.from_numpy(x)).numpy()
    target_mean = np.asarray(source.checkpoint["target_mean"], dtype=np.float32).reshape(1, len(TARGET_NAMES))
    target_std = np.asarray(source.checkpoint["target_std"], dtype=np.float32).reshape(1, len(TARGET_NAMES))
    y = y * target_std + target_mean
    y[:, 0:5] = np.maximum(y[:, 0:5], 0.0)
    y[:, 5:10] = np.maximum(y[:, 5:10], 0.0)
    return y


def estimate_pv_from_climate(
    climate: np.ndarray,
    lat: float,
    lon: float,
    angle_deg: float,
    aspect_deg: float,
    peak_power_kwp: float,
    loss_pct: float,
) -> dict[str, Any]:
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
            ghi, dni, dhi, temp, _wind = climate[idx, 0:5].astype(float)
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


class CoverageMask:
    def __init__(self, rows: list[dict[str, Any]]) -> None:
        self.rows = rows

    @classmethod
    def load(cls, path: Path) -> "CoverageMask":
        data = json.loads(path.expanduser().read_text(encoding="utf-8"))
        return cls(list(data["rows"]))

    def contains(self, lat: float, lon: float) -> bool:
        for row in self.rows:
            if float(row["lat_min"]) <= lat <= float(row["lat_max"]):
                return any(float(lo) <= lon <= float(hi) for lo, hi in row["lon_intervals"])
        return False


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
            request = urllib.request.Request(url, headers={"User-Agent": "pv-estimator-source-ensemble/0.1"})
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
    predictions: dict[str, dict[str, Any]],
    references: dict[str, dict[str, float]],
    sarah3_applicable: bool,
) -> dict[str, Any]:
    energy_values = np.array([value["energy_kwh"] for value in predictions.values()], dtype=np.float64)
    poa_values = np.array([value["poa_kwh_m2"] for value in predictions.values()], dtype=np.float64)
    ghi_values = np.array([value["ghi_kwh_m2"] for value in predictions.values()], dtype=np.float64)
    if len(energy_values) == 0:
        raise RuntimeError(f"no applicable source models for {location['location_id']}")
    energy_low = float(np.min(energy_values))
    energy_high = float(np.max(energy_values))
    energy_center = float(np.mean(energy_values))
    row: dict[str, Any] = {
        "location_id": location["location_id"],
        "name": location.get("name", ""),
        "region": location.get("region", ""),
        "latitude": float(location["latitude"]),
        "longitude": float(location["longitude"]),
        "applicable_sources": ",".join(predictions.keys()),
        "source_count": len(predictions),
        "ensemble_energy_kwh": energy_center,
        "ensemble_energy_low_kwh": energy_low,
        "ensemble_energy_high_kwh": energy_high,
        "ensemble_energy_half_spread_kwh": (energy_high - energy_low) / 2.0,
        "ensemble_energy_spread_pct": pct(energy_high - energy_low, energy_center),
        "ensemble_poa_kwh_m2": float(np.mean(poa_values)),
        "ensemble_poa_low_kwh_m2": float(np.min(poa_values)),
        "ensemble_poa_high_kwh_m2": float(np.max(poa_values)),
        "ensemble_ghi_kwh_m2": float(np.mean(ghi_values)),
        "pvgis_sarah3_applicable": sarah3_applicable,
    }
    for source_id, prediction in predictions.items():
        row[f"{source_id}_energy_kwh"] = prediction["energy_kwh"]
        row[f"{source_id}_poa_kwh_m2"] = prediction["poa_kwh_m2"]
        row[f"{source_id}_ghi_kwh_m2"] = prediction["ghi_kwh_m2"]
    for database, prefix in [("PVGIS-ERA5", "pvgis_era5"), ("PVGIS-SARAH3", "pvgis_sarah3")]:
        reference = references.get(database)
        if reference is None:
            continue
        row[f"{prefix}_reference_energy_kwh"] = reference["energy_kwh"]
        row[f"{prefix}_reference_poa_kwh_m2"] = reference["poa_kwh_m2"]
        row[f"{prefix}_reference_year_variability_kwh"] = reference["year_variability_kwh"]
        row[f"{prefix}_reference_energy_error_pct"] = pct(row["ensemble_energy_kwh"] - reference["energy_kwh"], reference["energy_kwh"])
    return row


def build_estimate_document(
    args: argparse.Namespace,
    location: dict[str, str],
    predictions: dict[str, dict[str, Any]],
    references: dict[str, dict[str, float]],
    sarah3_applicable: bool,
) -> dict[str, Any]:
    source_estimates = [source_estimate_document(source_id, prediction) for source_id, prediction in predictions.items()]
    monthly_estimates = []
    for month in range(1, 13):
        monthly_source_estimates = [
            monthly
            for prediction in predictions.values()
            for monthly in prediction["monthly"]
            if int(monthly["month"]) == month
        ]
        if monthly_source_estimates:
            monthly_estimates.append(
                {
                    "month": month,
                    "energy": energy_band(monthly["energy_kwh"] for monthly in monthly_source_estimates),
                    "in_plane_irradiation": irradiation_band(monthly["poa_kwh_m2"] for monthly in monthly_source_estimates),
                    "global_horizontal_irradiation": irradiation_band(monthly["ghi_kwh_m2"] for monthly in monthly_source_estimates),
                }
            )

    return {
        "schema_version": 1,
        "location": {
            "location_id": location["location_id"],
            "name": location.get("name", ""),
            "region": location.get("region", ""),
            "latitude": float(location["latitude"]),
            "longitude": float(location["longitude"]),
        },
        "system": {
            "peak_power_kwp": args.peak_power_kwp,
            "loss_pct": args.loss_pct,
            "tilt_deg": args.angle_deg,
            "aspect_deg": args.aspect_deg,
        },
        "coverage": {
            "pvgis_sarah3_applicable": sarah3_applicable,
            "applicable_sources": list(predictions.keys()),
        },
        "ensemble_estimate": {
            "source_estimates": source_estimates,
            "annual_energy": energy_band(prediction["energy_kwh"] for prediction in predictions.values()),
            "annual_in_plane_irradiation": irradiation_band(prediction["poa_kwh_m2"] for prediction in predictions.values()),
            "annual_global_horizontal_irradiation": irradiation_band(prediction["ghi_kwh_m2"] for prediction in predictions.values()),
            "monthly_estimates": monthly_estimates,
        },
        "references": reference_document(references),
    }


def source_estimate_document(source_id: str, prediction: dict[str, Any]) -> dict[str, Any]:
    return {
        "weather_source_id": source_id,
        "annual_energy": energy(prediction["energy_kwh"]),
        "annual_in_plane_irradiation": irradiation(prediction["poa_kwh_m2"]),
        "annual_global_horizontal_irradiation": irradiation(prediction["ghi_kwh_m2"]),
        "monthly_estimates": [
            {
                "month": int(monthly["month"]),
                "energy": energy(monthly["energy_kwh"]),
                "in_plane_irradiation": irradiation(monthly["poa_kwh_m2"]),
                "global_horizontal_irradiation": irradiation(monthly["ghi_kwh_m2"]),
            }
            for monthly in prediction["monthly"]
        ],
    }


def reference_document(references: dict[str, dict[str, float]]) -> dict[str, Any]:
    return {
        database: {
            "annual_energy": energy(reference["energy_kwh"]),
            "annual_in_plane_irradiation": irradiation(reference["poa_kwh_m2"]),
            "year_variability": energy(reference["year_variability_kwh"]),
            "total_loss_pct": reference["total_loss_pct"],
            "monthly_energy_kwh": reference["monthly_energy_kwh"],
            "monthly_in_plane_irradiation_kwh_m2": reference["monthly_poa_kwh_m2"],
        }
        for database, reference in references.items()
    }


def energy(kwh: float) -> dict[str, float]:
    return {"watt_hours": float(kwh) * 1000.0}


def irradiation(kwh_m2: float) -> dict[str, float]:
    return {"kilowatt_hours_per_square_meter": float(kwh_m2)}


def energy_band(values: Any) -> dict[str, Any]:
    band = scalar_band(values)
    return {
        "mean": energy(band["mean"]),
        "low": energy(band["low"]),
        "high": energy(band["high"]),
        "half_spread": energy(band["half_spread"]),
        "spread_fraction": band["spread_fraction"],
    }


def irradiation_band(values: Any) -> dict[str, Any]:
    band = scalar_band(values)
    return {
        "mean": irradiation(band["mean"]),
        "low": irradiation(band["low"]),
        "high": irradiation(band["high"]),
        "half_spread": irradiation(band["half_spread"]),
        "spread_fraction": band["spread_fraction"],
    }


def scalar_band(values: Any) -> dict[str, float]:
    data = np.asarray(list(values), dtype=np.float64)
    if len(data) == 0:
        raise RuntimeError("estimate bands require at least one source value")
    low = float(np.min(data))
    high = float(np.max(data))
    mean = float(np.mean(data))
    spread = high - low
    return {
        "mean": mean,
        "low": low,
        "high": high,
        "half_spread": spread / 2.0,
        "spread_fraction": 0.0 if mean == 0.0 else spread / mean,
    }


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


def summarize(args: argparse.Namespace, rows: list[dict[str, Any]], sources: list[LoadedSource]) -> dict[str, Any]:
    summary: dict[str, Any] = {
        "locations": len(rows),
        "system": {
            "peak_power_kwp": args.peak_power_kwp,
            "loss_pct": args.loss_pct,
            "angle_deg": args.angle_deg,
            "aspect_deg": args.aspect_deg,
        },
        "sources": {
            source.spec.source_id: {
                "label": source.spec.label,
                "checkpoint": str(source.spec.checkpoint),
                "coverage": source.spec.coverage,
                "epoch": source.checkpoint.get("epoch"),
                "location_count": source.checkpoint.get("location_count"),
                "parameters": source.checkpoint.get("parameters"),
            }
            for source in sources
        },
    }
    for key in ["source_count", "ensemble_energy_kwh", "ensemble_energy_half_spread_kwh", "ensemble_energy_spread_pct"]:
        values = np.array([float(row[key]) for row in rows if row.get(key) not in ("", None)], dtype=np.float64)
        if len(values) > 0:
            summary[key] = summarize_values(values)
    for prefix in ["pvgis_era5", "pvgis_sarah3"]:
        errors = np.array([float(row[f"{prefix}_reference_energy_error_pct"]) for row in rows if row.get(f"{prefix}_reference_energy_error_pct") not in ("", None)], dtype=np.float64)
        if len(errors) > 0:
            summary[f"{prefix}_reference"] = {
                "count": int(len(errors)),
                "energy_mbe_pct": float(np.mean(errors)),
                "energy_mae_pct": float(np.mean(np.abs(errors))),
                "energy_rmse_pct": float(np.sqrt(np.mean(errors * errors))),
            }
    return summary


def summarize_values(values: np.ndarray) -> dict[str, float]:
    return {
        "count": int(len(values)),
        "mean": float(np.mean(values)),
        "median": float(np.median(values)),
        "min": float(np.min(values)),
        "max": float(np.max(values)),
    }


def print_summary(summary: dict[str, Any]) -> None:
    print(
        f"ensemble_energy: mean={summary['ensemble_energy_kwh']['mean']:.2f} "
        f"spread_mean={summary['ensemble_energy_spread_pct']['mean']:.2f}%",
        flush=True,
    )
    for key in ["pvgis_era5_reference", "pvgis_sarah3_reference"]:
        if key not in summary:
            continue
        value = summary[key]
        print(
            f"{key}: n={value['count']} mbe={value['energy_mbe_pct']:.2f}% "
            f"mae={value['energy_mae_pct']:.2f}% rmse={value['energy_rmse_pct']:.2f}%",
            flush=True,
        )


def safe_name(value: str) -> str:
    return "".join(char.lower() if char.isalnum() else "_" for char in value).strip("_")


if __name__ == "__main__":
    raise SystemExit(main())
