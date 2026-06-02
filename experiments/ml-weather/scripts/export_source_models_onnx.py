#!/usr/bin/env python3
"""Export trained source climate-normal checkpoints to ONNX artifacts."""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import numpy as np
import torch
from onnxruntime.quantization import QuantType, quantize_dynamic

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

from infer_source_ensemble import estimate_pv_from_climate  # noqa: E402
from train_climate_normals_stream_torch import ClimateNormalMlp, INPUT_FEATURES, TARGET_NAMES, encode_features  # noqa: E402

TEMPORAL_BINS = 288
UNCERTAINTY_MULTIPLIER = 2.0
REFERENCE_POINTS = [
    ("it_potenza", 40.650, 15.643),
    ("it_arezzo", 43.707, 11.916),
    ("es_madrid", 40.4168, -3.7038),
]


@dataclass(frozen=True)
class SourceExport:
    source_id: str
    label: str
    checkpoint: Path
    coverage_rule: dict[str, Any]
    onnx_name: str


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--nasa-checkpoint", type=Path, required=True)
    parser.add_argument("--era5-checkpoint", type=Path, required=True)
    parser.add_argument("--sarah3-checkpoint", type=Path, required=True)
    parser.add_argument("--sarah3-mask", type=Path, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("--opset", type=int, default=17)
    parser.add_argument("--fp32-parity-atol", type=float, default=1e-4)
    parser.add_argument("--int8-reference-mae-pct", type=float, default=0.25)
    parser.add_argument("--keep-fp32", action="store_true", help="keep intermediate FP32 ONNX files beside INT8 artifacts")
    parser.add_argument("--skip-parity", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)
    coverage_dir = args.out_dir / "coverage"
    coverage_dir.mkdir(parents=True, exist_ok=True)
    mask_target = coverage_dir / "pvgis_sarah3_empirical_grid_mask.json"
    shutil.copyfile(args.sarah3_mask.expanduser(), mask_target)

    sources = [
        SourceExport(
            "nasa_power",
            "NASA POWER",
            args.nasa_checkpoint,
            {"type": "global"},
            "nasa_power.onnx",
        ),
        SourceExport(
            "pvgis_era5",
            "PVGIS-ERA5",
            args.era5_checkpoint,
            {"type": "global_land_pvgis_gateway"},
            "pvgis_era5.onnx",
        ),
        SourceExport(
            "pvgis_sarah3",
            "PVGIS-SARAH3",
            args.sarah3_checkpoint,
            {"type": "empirical_grid_mask", "mask_path": str(mask_target.relative_to(args.out_dir))},
            "pvgis_sarah3.onnx",
        ),
    ]

    fp32_dir = args.out_dir / "_fp32"
    fp32_dir.mkdir(parents=True, exist_ok=True)
    manifest_sources = []
    parity = {}
    for source in sources:
        model, checkpoint = load_checkpoint(source.checkpoint)
        fp32_path = fp32_dir / source.onnx_name.replace(".onnx", ".fp32.onnx")
        int8_path = args.out_dir / source.onnx_name
        export_onnx(model, fp32_path, args.opset)
        quantize_int8(fp32_path, int8_path)
        if not args.skip_parity:
            parity[source.source_id] = {
                "fp32": check_fp32_parity(model, fp32_path, args.fp32_parity_atol),
                "int8": check_int8_delta(
                    model,
                    fp32_path,
                    int8_path,
                    checkpoint,
                    args.int8_reference_mae_pct,
                ),
            }
        source_manifest = {
            "source_id": source.source_id,
            "label": source.label,
            "active": True,
            "onnx_path": source.onnx_name,
            "sha256": sha256_file(int8_path),
            "input_name": "features",
            "output_name": "normalized_targets",
            "coverage_rule": source.coverage_rule,
            "target_mean": list(map(float, checkpoint["target_mean"])),
            "target_std": list(map(float, checkpoint["target_std"])),
            "parameters": int(checkpoint.get("parameters", 0)),
            "epoch": int(checkpoint.get("epoch", 0)),
            "location_count": int(checkpoint.get("location_count", checkpoint.get("locations", 0) or 0)),
            "checkpoint_path": str(source.checkpoint),
            "checkpoint_sha256": sha256_file(source.checkpoint.expanduser()),
            "fp32_onnx_bytes": fp32_path.stat().st_size,
            "int8_onnx_bytes": int8_path.stat().st_size,
            "int8_over_fp32": int8_path.stat().st_size / fp32_path.stat().st_size,
        }
        if args.keep_fp32:
            source_manifest["fp32_onnx_path"] = str(fp32_path.relative_to(args.out_dir))
            source_manifest["fp32_onnx_sha256"] = sha256_file(fp32_path)
        manifest_sources.append(source_manifest)

    if not args.keep_fp32:
        shutil.rmtree(fp32_dir)

    manifest = {
        "schema_version": 1,
        "model_family": "monthly-hourly climate-normal residual MLP",
        "input_features": INPUT_FEATURES,
        "temporal_bins": TEMPORAL_BINS,
        "target_names": TARGET_NAMES,
        "uncertainty_multiplier": UNCERTAINTY_MULTIPLIER,
        "quantization": {
            "format": "onnx_dynamic_qint8",
            "weight_type": "qint8",
            "per_channel": False,
            "op_types": ["MatMul", "Gemm"],
        },
        "sources": manifest_sources,
        "parity": parity,
    }
    manifest_path = args.out_dir / "source-model-artifacts.json"
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {manifest_path}", flush=True)
    return 0


def load_checkpoint(path: Path) -> tuple[ClimateNormalMlp, dict[str, Any]]:
    checkpoint = torch.load(path.expanduser(), map_location="cpu", weights_only=False)
    model = ClimateNormalMlp(
        int(checkpoint["input_features"]),
        int(checkpoint["hidden_width"]),
        int(checkpoint["residual_blocks"]),
        float(checkpoint["residual_scale"]),
    )
    model.load_state_dict(checkpoint["model_state_dict"])
    model.eval()
    return model, checkpoint


def export_onnx(model: ClimateNormalMlp, out: Path, opset: int) -> None:
    dummy = torch.zeros((TEMPORAL_BINS, INPUT_FEATURES), dtype=torch.float32)
    torch.onnx.export(
        model,
        dummy,
        out,
        input_names=["features"],
        output_names=["normalized_targets"],
        dynamic_axes={"features": {0: "rows"}, "normalized_targets": {0: "rows"}},
        opset_version=opset,
    )
    print(f"wrote {out}", flush=True)


def quantize_int8(fp32_path: Path, int8_path: Path) -> None:
    quantize_dynamic(
        model_input=fp32_path,
        model_output=int8_path,
        weight_type=QuantType.QInt8,
        per_channel=False,
        op_types_to_quantize=["MatMul", "Gemm"],
    )
    print(f"wrote {int8_path}", flush=True)


def check_fp32_parity(model: ClimateNormalMlp, onnx_path: Path, atol: float) -> dict[str, float]:
    import onnxruntime as ort

    session = ort.InferenceSession(str(onnx_path), providers=["CPUExecutionProvider"])
    max_abs = 0.0
    mean_abs = []
    with torch.no_grad():
        for _name, lat, lon in REFERENCE_POINTS:
            features = reference_features(lat, lon)
            torch_out = model(torch.from_numpy(features)).numpy()
            onnx_out = session.run(["normalized_targets"], {"features": features})[0]
            diff = np.abs(torch_out - onnx_out)
            max_abs = max(max_abs, float(np.max(diff)))
            mean_abs.append(float(np.mean(diff)))
    if max_abs > atol:
        raise RuntimeError(f"FP32 ONNX parity failed for {onnx_path}: max_abs={max_abs} > {atol}")
    return {"max_abs": max_abs, "mean_abs": float(np.mean(mean_abs)), "points": len(REFERENCE_POINTS)}


def check_int8_delta(
    model: ClimateNormalMlp,
    fp32_path: Path,
    int8_path: Path,
    checkpoint: dict[str, Any],
    annual_mae_limit_pct: float,
) -> dict[str, float]:
    import onnxruntime as ort

    fp32_session = ort.InferenceSession(str(fp32_path), providers=["CPUExecutionProvider"])
    int8_session = ort.InferenceSession(str(int8_path), providers=["CPUExecutionProvider"])
    normalized_max_abs = 0.0
    annual_energy_deltas = []
    for _name, lat, lon in REFERENCE_POINTS:
        features = reference_features(lat, lon)
        fp32_out = fp32_session.run(["normalized_targets"], {"features": features})[0]
        int8_out = int8_session.run(["normalized_targets"], {"features": features})[0]
        normalized_max_abs = max(normalized_max_abs, float(np.max(np.abs(fp32_out - int8_out))))
        fp32_pv = estimate_pv_from_climate(denormalize(fp32_out, checkpoint), lat, lon, 30.0, 0.0, 1.0, 14.0)
        int8_pv = estimate_pv_from_climate(denormalize(int8_out, checkpoint), lat, lon, 30.0, 0.0, 1.0, 14.0)
        annual_energy_deltas.append(
            100.0 * (int8_pv["energy_kwh"] - fp32_pv["energy_kwh"]) / fp32_pv["energy_kwh"]
        )
    annual_mae_pct = float(np.mean(np.abs(annual_energy_deltas)))
    if annual_mae_pct > annual_mae_limit_pct:
        raise RuntimeError(
            f"INT8 reference annual MAE {annual_mae_pct:.4f}% exceeds {annual_mae_limit_pct:.4f}% for {int8_path}"
        )
    return {
        "normalized_max_abs": normalized_max_abs,
        "annual_energy_mae_pct": annual_mae_pct,
        "annual_energy_max_abs_pct": float(np.max(np.abs(annual_energy_deltas))),
        "points": len(REFERENCE_POINTS),
    }


def reference_features(lat: float, lon: float) -> np.ndarray:
    temporal = np.arange(TEMPORAL_BINS, dtype=np.int64)
    return encode_features(
        np.full(TEMPORAL_BINS, lat, dtype=np.float32),
        np.full(TEMPORAL_BINS, lon, dtype=np.float32),
        temporal,
        TEMPORAL_BINS,
    )


def denormalize(values: np.ndarray, checkpoint: dict[str, Any]) -> np.ndarray:
    target_mean = np.asarray(checkpoint["target_mean"], dtype=np.float32).reshape(1, len(TARGET_NAMES))
    target_std = np.asarray(checkpoint["target_std"], dtype=np.float32).reshape(1, len(TARGET_NAMES))
    climate = values * target_std + target_mean
    climate[:, 0:5] = np.maximum(climate[:, 0:5], 0.0)
    climate[:, 5:10] = np.maximum(climate[:, 5:10], 0.0)
    return climate


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


if __name__ == "__main__":
    raise SystemExit(main())
