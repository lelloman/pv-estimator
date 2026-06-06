# Source Model Reproduction Runbook

This directory contains the retained rebuild path for the source-model bundle used by PV Estimator v0.1.0. It is not needed to run the CLI or TUI. Normal users use the embedded INT8 ONNX bundle, or the public copy at:

```text
https://huggingface.co/lelloman/pv-estimator-tight-v1-int8
```

The large hourly datasets, climate-normal arrays, PyTorch checkpoints, and generated run outputs are intentionally not committed.

## Shipped Bundle

The v0.1.0 runtime bundle is `artifacts/source-models-768x8-int8`. It contains three dynamically quantized INT8 ONNX models and the SARAH3 empirical coverage mask.

| Source | Training locations | Training rows | Best epoch | Validation MAE | Checkpoint |
| --- | ---: | ---: | ---: | ---: | --- |
| NASA POWER | 7,056 | 309,391,488 | 77 | 5.101154 | `results/full_climate_normals_compressor_holdout_768x8/best_model.pt` |
| PVGIS-ERA5 tight v1 | 2,825 | 470,464,204 | 77 | 7.124548 | `results/pvgis_era5_climate_normals_compressor_768x8_tight_v1/best_model.pt` |
| PVGIS-SARAH3 tight v1 | 1,449 | 241,310,667 | 73 | 6.651263 | `results/pvgis_sarah3_climate_normals_compressor_768x8_tight_v1/best_model.pt` |

All three source models use the same architecture:

```text
model family: monthly-hourly climate-normal residual MLP
input features: 66 Fourier/cross features from latitude, longitude, month, and hour
outputs: 10 monthly-hourly climate-normal targets
hidden width: 768
residual blocks: 8
residual scale: 0.5
parameter dtype before export: float32
runtime format: ONNX dynamic QInt8, MatMul/Gemm weights, per-tensor quantization
```

The authoritative metadata is in `config/source_model_registry.json` and `artifacts/source-models-768x8-int8/source-model-artifacts.json`.

## Retained Files

The committed reproduction tree keeps only the files required to rebuild, export, or validate the shipped source-model bundle:

```text
reproduction/source-models/config/
reproduction/source-models/scripts/download_nasa_power.py
reproduction/source-models/scripts/download_pvgis_series.py
reproduction/source-models/scripts/normalize_nasa_power.py
reproduction/source-models/scripts/normalize_pvgis_series.py
reproduction/source-models/scripts/build_climate_normals.rs
reproduction/source-models/scripts/train_climate_normals_stream_torch.py
reproduction/source-models/scripts/infer_source_ensemble.py
reproduction/source-models/scripts/export_source_models_onnx.py
reproduction/source-models/results/2026-06-05_tight_v1_int8_comparison.md
```

Older exploratory scripts, architecture sweeps, geography-v2 probes, NSRDB scaffolding, and intermediate result notes were removed from the release tree.

## Environment

Training was originally done on `rtx.homelab` under:

```text
~/pv-estimator-gpu
```

The scripts require Python 3 plus PyTorch, NumPy, ONNX, and ONNX Runtime. Install the minimal Python dependencies in your training environment:

```sh
python3 -m venv .venv
.venv/bin/pip install -r reproduction/source-models/requirements.txt
```

The trainer automatically uses CUDA when available.

## Rebuild Hourly Source CSVs

Generated data should go under `reproduction/source-models/runs/`, which is ignored by git.

### NASA POWER

Download the 2020-2024 global grid:

```sh
python3 reproduction/source-models/scripts/download_nasa_power.py   --locations reproduction/source-models/config/global_grid_7056_locations_shuffled.csv   --out-dir reproduction/source-models/runs/global_grid_7056/raw/nasa_power_hourly   --start 20200101   --end 20241231   --workers 4   --timeout-seconds 60   --retries 2   --request-delay-seconds 1   --request-jitter-seconds 2
```

Normalize with the Rust normalizer:

```sh
cargo run --release -p xtask -- normalize-nasa-power   --raw-dir reproduction/source-models/runs/global_grid_7056/raw/nasa_power_hourly   --out reproduction/source-models/runs/global_grid_7056/normalized/nasa_power_hourly.csv.gz   --workers 16   --pigz-threads 1
```

### PVGIS-ERA5 Tight v1

The tight-v1 ERA5 set combines the original source-ensemble ERA5 list and the tight-v1 add-on list. Run both downloads into the same raw directory so normalization sees one source dataset:

```sh
python3 reproduction/source-models/scripts/download_pvgis_series.py   --locations reproduction/source-models/config/source_coverage/pvgis_era5_locations.csv   --out-dir reproduction/source-models/runs/pvgis_era5_tight_v1/raw   --databases PVGIS-ERA5   --start-year 2005   --end-year 2023   --workers 1   --request-delay-seconds 2   --request-jitter-seconds 1   --timeout-seconds 240

python3 reproduction/source-models/scripts/download_pvgis_series.py   --locations reproduction/source-models/config/source_coverage/pvgis_era5_tight_new_locations_v1.csv   --out-dir reproduction/source-models/runs/pvgis_era5_tight_v1/raw   --databases PVGIS-ERA5   --start-year 2005   --end-year 2023   --workers 1   --request-delay-seconds 2   --request-jitter-seconds 1   --timeout-seconds 240

python3 reproduction/source-models/scripts/normalize_pvgis_series.py   --raw-dir reproduction/source-models/runs/pvgis_era5_tight_v1/raw   --out reproduction/source-models/runs/pvgis_era5_tight_v1/normalized/pvgis_era5.csv.gz
```

### PVGIS-SARAH3 Tight v1

The tight-v1 SARAH3 set follows the same pattern:

```sh
python3 reproduction/source-models/scripts/download_pvgis_series.py   --locations reproduction/source-models/config/source_coverage/pvgis_sarah3_locations.csv   --out-dir reproduction/source-models/runs/pvgis_sarah3_tight_v1/raw   --databases PVGIS-SARAH3   --start-year 2005   --end-year 2023   --workers 1   --request-delay-seconds 2   --request-jitter-seconds 1   --timeout-seconds 240

python3 reproduction/source-models/scripts/download_pvgis_series.py   --locations reproduction/source-models/config/source_coverage/pvgis_sarah3_tight_new_locations_v1.csv   --out-dir reproduction/source-models/runs/pvgis_sarah3_tight_v1/raw   --databases PVGIS-SARAH3   --start-year 2005   --end-year 2023   --workers 1   --request-delay-seconds 2   --request-jitter-seconds 1   --timeout-seconds 240

python3 reproduction/source-models/scripts/normalize_pvgis_series.py   --raw-dir reproduction/source-models/runs/pvgis_sarah3_tight_v1/raw   --out reproduction/source-models/runs/pvgis_sarah3_tight_v1/normalized/pvgis_sarah3.csv.gz
```

## Build Climate-Normal Tables

Compile the standalone normals builder:

```sh
rustc -O reproduction/source-models/scripts/build_climate_normals.rs -o reproduction/source-models/runs/build_climate_normals
```

Build the three monthly-hour normal tables:

```sh
reproduction/source-models/runs/build_climate_normals   --data reproduction/source-models/runs/global_grid_7056/normalized/nasa_power_hourly.csv.gz   --out-dir reproduction/source-models/runs/normals/full_climate_normals_7056   --temporal-bins month-hour   --progress-every 10000000

reproduction/source-models/runs/build_climate_normals   --data reproduction/source-models/runs/pvgis_era5_tight_v1/normalized/pvgis_era5.csv.gz   --out-dir reproduction/source-models/runs/normals/pvgis_era5_climate_normals_2005_2023_tight_v1   --temporal-bins month-hour   --progress-every 10000000

reproduction/source-models/runs/build_climate_normals   --data reproduction/source-models/runs/pvgis_sarah3_tight_v1/normalized/pvgis_sarah3.csv.gz   --out-dir reproduction/source-models/runs/normals/pvgis_sarah3_climate_normals_2005_2023_tight_v1   --temporal-bins month-hour   --progress-every 10000000
```

Each normals directory contains `climate_normals.npy`, `climate_normal_std.npy`, `climate_normal_counts.npy`, `location_keys.npy`, and `metadata.json`.

## Retrain Checkpoints

Train the three 768x8 source models:

```sh
.venv/bin/python reproduction/source-models/scripts/train_climate_normals_stream_torch.py   --normals-dir reproduction/source-models/runs/normals/full_climate_normals_7056   --out-dir reproduction/source-models/runs/results/full_climate_normals_compressor_holdout_768x8   --hidden-width 768   --residual-blocks 8   --residual-scale 0.5   --epochs 100   --batch-size 65536   --learning-rate 0.0007   --val-location-stride 17   --eval-every 1   --eval-batch-size 262144

.venv/bin/python reproduction/source-models/scripts/train_climate_normals_stream_torch.py   --normals-dir reproduction/source-models/runs/normals/pvgis_era5_climate_normals_2005_2023_tight_v1   --out-dir reproduction/source-models/runs/results/pvgis_era5_climate_normals_compressor_768x8_tight_v1   --hidden-width 768   --residual-blocks 8   --residual-scale 0.5   --epochs 80   --batch-size 65536   --learning-rate 0.001   --val-location-stride 17   --eval-every 1   --eval-batch-size 262144

.venv/bin/python reproduction/source-models/scripts/train_climate_normals_stream_torch.py   --normals-dir reproduction/source-models/runs/normals/pvgis_sarah3_climate_normals_2005_2023_tight_v1   --out-dir reproduction/source-models/runs/results/pvgis_sarah3_climate_normals_compressor_768x8_tight_v1   --hidden-width 768   --residual-blocks 8   --residual-scale 0.5   --epochs 80   --batch-size 65536   --learning-rate 0.001   --val-location-stride 17   --eval-every 1   --eval-batch-size 262144
```

Expected outputs in each result directory are `best_model.pt`, `model.pt`, and `metrics.json`.

## Export INT8 ONNX Bundle

Export and dynamically quantize the runtime bundle:

```sh
.venv/bin/python reproduction/source-models/scripts/export_source_models_onnx.py   --nasa-checkpoint reproduction/source-models/runs/results/full_climate_normals_compressor_holdout_768x8/best_model.pt   --era5-checkpoint reproduction/source-models/runs/results/pvgis_era5_climate_normals_compressor_768x8_tight_v1/best_model.pt   --sarah3-checkpoint reproduction/source-models/runs/results/pvgis_sarah3_climate_normals_compressor_768x8_tight_v1/best_model.pt   --sarah3-mask reproduction/source-models/config/source_coverage/pvgis_sarah3_empirical_grid_mask.json   --out-dir reproduction/source-models/runs/artifacts/source-models-768x8-int8
```

The exported directory contains:

```text
source-model-artifacts.json
nasa_power.onnx
pvgis_era5.onnx
pvgis_sarah3.onnx
coverage/pvgis_sarah3_empirical_grid_mask.json
```

## Validate

Run the source-ensemble benchmark against PVGIS references:

```sh
.venv/bin/python reproduction/source-models/scripts/infer_source_ensemble.py   --locations reproduction/source-models/config/regional_benchmark_cities_120.csv   --fetch-pvgis   --sarah3-mask reproduction/source-models/config/source_coverage/pvgis_sarah3_empirical_grid_mask.json   --nasa-checkpoint reproduction/source-models/runs/results/full_climate_normals_compressor_holdout_768x8/best_model.pt   --era5-checkpoint reproduction/source-models/runs/results/pvgis_era5_climate_normals_compressor_768x8_tight_v1/best_model.pt   --sarah3-checkpoint reproduction/source-models/runs/results/pvgis_sarah3_climate_normals_compressor_768x8_tight_v1/best_model.pt   --out-csv reproduction/source-models/runs/validation/regional_cities_120.csv   --out-json reproduction/source-models/runs/validation/regional_cities_120.summary.json   --out-estimate-json reproduction/source-models/runs/validation/regional_cities_120.estimates.json
```

The shipped tight-v1 INT8 bundle measured:

```text
regional 120 vs PVGIS-ERA5:   MAE 2.470%, RMSE 3.201%
regional 100 vs PVGIS-SARAH3: MAE 3.218%, RMSE 4.418%
mean source disagreement spread: 4.785%
```

The exact numbers can move if upstream PVGIS/NASA data changes, PyTorch/CUDA versions differ, or the training seed changes. A rebuilt bundle should stay in the same error range and should reproduce the artifact hash and validation story in `results/2026-06-05_tight_v1_int8_comparison.md` when built from the same data and checkpoints.

## Artifact Policy

Commit small reproduction inputs, scripts, and reports. Do not commit generated raw JSON, normalized CSVs, `.npy` climate-normal arrays, PyTorch `.pt` checkpoints, generated ONNX bundles, or benchmark output under `runs/`.
