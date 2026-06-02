# Source Model Reproduction Runbook

Date: 2026-06-01

This is the rebuild path for the three active annual/monthly source models if the
trained `.pt` checkpoints on `rtx.homelab` are lost.

The large source data and checkpoints are intentionally not committed. This file
records the checked-in scripts, expected intermediate artifacts, commands, and
known metrics needed to recreate them.

## Active Models

| Source | Checkpoint | Params | Checkpoint size | Best epoch | Validation MAE |
| --- | --- | ---: | ---: | ---: | ---: |
| NASA POWER | `results/full_climate_normals_compressor_holdout_768x8/best_model.pt` | 9,522,442 | 36.34 MiB | 77 | 5.101154 |
| PVGIS-ERA5 | `results/pvgis_era5_climate_normals_compressor_768x8/best_model.pt` | 9,522,442 | 36.34 MiB | 75 | 8.333487 |
| PVGIS-SARAH3 | `results/pvgis_sarah3_climate_normals_compressor_768x8/best_model.pt` | 9,522,442 | 36.34 MiB | 80 | 7.889470 |

All three models use the same architecture:

```text
model family: monthly-hourly climate-normal residual MLP
input features: 66 Fourier/cross features from lat, lon, month, hour
outputs: 10 values
  GHI mean, DNI mean, DHI mean, temperature mean, wind mean
  GHI std, DNI std, DHI std, temperature std, wind std
hidden width: 768
residual blocks: 8
residual scale: 0.5
parameter dtype: float32
```

## Required Scripts

These scripts are small and should be committed:

```text
experiments/ml-weather/scripts/download_nasa_power.py
experiments/ml-weather/scripts/download_pvgis_series.py
experiments/ml-weather/scripts/normalize_nasa_power.py
experiments/ml-weather/scripts/normalize_pvgis_series.py
experiments/ml-weather/scripts/build_climate_normals.rs
experiments/ml-weather/scripts/train_climate_normals_stream_torch.py
experiments/ml-weather/scripts/infer_source_ensemble.py
experiments/ml-weather/scripts/export_source_models_onnx.py
experiments/ml-weather/scripts/build_source_coverage_lists.py
experiments/ml-weather/scripts/generate_source_ensemble_locations.py
```

The production Rust crates do not depend on these scripts. They are experiment
and rebuild tooling.

## Required Configs

These small configs/location lists should be committed:

```text
experiments/ml-weather/config/global_grid_7056_locations.csv
experiments/ml-weather/config/global_grid_7056_locations_shuffled.csv
experiments/ml-weather/config/source_ensemble_locations_2000.csv
experiments/ml-weather/config/source_coverage/pvgis_era5_locations.csv
experiments/ml-weather/config/source_coverage/pvgis_sarah3_locations.csv
experiments/ml-weather/config/source_coverage/pvgis_sarah3_empirical_grid_mask.json
experiments/ml-weather/config/pvgis_source_databases.json
experiments/ml-weather/config/source_model_registry.json
```

## Environment

Training was done on `rtx.homelab` under:

```text
~/pv-estimator-gpu
```

Use the Python virtualenv there:

```sh
cd ~/pv-estimator-gpu
.venv/bin/python -c "import torch; print(torch.__version__, torch.cuda.is_available())"
```

The climate-normal trainer automatically uses CUDA if available.

## Data Artifacts

The final source CSVs used for the current models were:

| Source | Locations | Rows | Path on RTX |
| --- | ---: | ---: | --- |
| NASA POWER | 7,056 | 309,391,488 | `data/nasa_power_hourly_global_grid_7056.csv.gz` |
| PVGIS-ERA5 | 1,972 | 328,408,996 | `data/pvgis_source_ensemble/pvgis_era5_full_2005_2023.csv.gz` |
| PVGIS-SARAH3 | 787 | 131,063,835 | `data/pvgis_source_ensemble/pvgis_sarah3_full_2005_2023.csv.gz` |

The final climate-normal tables are smaller and are enough to retrain the models
without re-downloading raw hourly data:

```text
data/full_climate_normals_7056/
data/pvgis_era5_climate_normals_2005_2023/
data/pvgis_sarah3_climate_normals_2005_2023/
```

Each normals directory contains:

```text
climate_normals.npy        shape: locations x 288 x 5, float32 means
climate_normal_std.npy     shape: locations x 288 x 5, float32 std devs
climate_normal_counts.npy  shape: locations x 288, int32 counts
location_keys.npy          packed lat/lon keys
metadata.json              source rows, skipped rows, location count
```

## Rebuild From Existing Hourly CSVs

Compile the standalone Rust normals builder on a machine with Rust:

```sh
cd ~/pv-estimator-gpu
rustc -O scripts/build_climate_normals.rs -o build_climate_normals
```

Build NASA POWER monthly-hour normal tables:

```sh
./build_climate_normals   --data data/nasa_power_hourly_global_grid_7056.csv.gz   --out-dir data/full_climate_normals_7056   --temporal-bins month-hour   --progress-every 10000000
```

Build PVGIS-ERA5 monthly-hour normal tables:

```sh
./build_climate_normals   --data data/pvgis_source_ensemble/pvgis_era5_full_2005_2023.csv.gz   --out-dir data/pvgis_era5_climate_normals_2005_2023   --temporal-bins month-hour   --progress-every 10000000
```

Build PVGIS-SARAH3 monthly-hour normal tables:

```sh
./build_climate_normals   --data data/pvgis_source_ensemble/pvgis_sarah3_full_2005_2023.csv.gz   --out-dir data/pvgis_sarah3_climate_normals_2005_2023   --temporal-bins month-hour   --progress-every 10000000
```

## Retrain From Climate-Normal Tables

Train NASA POWER 768x8:

```sh
cd ~/pv-estimator-gpu
.venv/bin/python scripts/train_climate_normals_stream_torch.py   --normals-dir data/full_climate_normals_7056   --out-dir results/full_climate_normals_compressor_holdout_768x8   --hidden-width 768   --residual-blocks 8   --residual-scale 0.5   --epochs 100   --batch-size 65536   --learning-rate 0.0007   --val-location-stride 17   --eval-every 1   --eval-batch-size 262144
```

Train PVGIS-ERA5 768x8:

```sh
cd ~/pv-estimator-gpu
.venv/bin/python scripts/train_climate_normals_stream_torch.py   --normals-dir data/pvgis_era5_climate_normals_2005_2023   --out-dir results/pvgis_era5_climate_normals_compressor_768x8   --hidden-width 768   --residual-blocks 8   --residual-scale 0.5   --epochs 80   --batch-size 65536   --learning-rate 0.001   --val-location-stride 17   --eval-every 1   --eval-batch-size 262144
```

Train PVGIS-SARAH3 768x8:

```sh
cd ~/pv-estimator-gpu
.venv/bin/python scripts/train_climate_normals_stream_torch.py   --normals-dir data/pvgis_sarah3_climate_normals_2005_2023   --out-dir results/pvgis_sarah3_climate_normals_compressor_768x8   --hidden-width 768   --residual-blocks 8   --residual-scale 0.5   --epochs 80   --batch-size 65536   --learning-rate 0.001   --val-location-stride 17   --eval-every 1   --eval-batch-size 262144
```

Expected outputs in each result directory:

```text
best_model.pt   best checkpoint by validation MAE
model.pt        final epoch checkpoint
metrics.json    full training history and metadata
```

## Rebuild PVGIS Hourly CSVs If Those Are Lost Too

The PVGIS source data can be regenerated from the checked-in coverage lists.
Keep one worker per machine and use cached raw files/error files for resumability.

ERA5 full collection:

```sh
python3 experiments/ml-weather/scripts/download_pvgis_series.py   --locations experiments/ml-weather/config/source_coverage/pvgis_era5_locations.csv   --out-dir experiments/ml-weather/runs/pvgis_era5_full/raw   --databases PVGIS-ERA5   --start-year 2005   --end-year 2023   --workers 1   --request-delay-seconds 2   --request-jitter-seconds 1   --timeout-seconds 240

python3 experiments/ml-weather/scripts/normalize_pvgis_series.py   --raw-dir experiments/ml-weather/runs/pvgis_era5_full/raw   --out experiments/ml-weather/runs/pvgis_era5_full/normalized/pvgis_era5.csv.gz
```

SARAH3 full collection:

```sh
python3 experiments/ml-weather/scripts/download_pvgis_series.py   --locations experiments/ml-weather/config/source_coverage/pvgis_sarah3_locations.csv   --out-dir experiments/ml-weather/runs/pvgis_sarah3_full/raw   --databases PVGIS-SARAH3   --start-year 2005   --end-year 2023   --workers 1   --request-delay-seconds 2   --request-jitter-seconds 1   --timeout-seconds 240

python3 experiments/ml-weather/scripts/normalize_pvgis_series.py   --raw-dir experiments/ml-weather/runs/pvgis_sarah3_full/raw   --out experiments/ml-weather/runs/pvgis_sarah3_full/normalized/pvgis_sarah3.csv.gz
```

For the current run, final PVGIS CSVs were assembled from normalized shards from
`vps-eu`, `vps-us`, and the local machine. If repeating distributed collection,
concatenate normalized CSVs carefully: keep one header and skip repeated headers.
The current assembled-row counts are listed above and in:

```text
experiments/ml-weather/results/2026-06-01_source_models_768x8.md
```

## Rebuild NASA POWER Hourly CSV If Lost

The NASA POWER model used the checked-in 7,056-location grid and the existing
NASA downloader/normalizer. The original normalized CSV path on RTX was:

```text
data/nasa_power_hourly_global_grid_7056.csv.gz
```

Regeneration path:

```sh
python3 experiments/ml-weather/scripts/download_nasa_power.py   --locations experiments/ml-weather/config/global_grid_7056_locations_shuffled.csv   --out-dir experiments/ml-weather/runs/global_grid_7056/raw/nasa_power_hourly   --start 20200101   --end 20241231   --workers 4   --timeout-seconds 60   --retries 2   --request-delay-seconds 1   --request-jitter-seconds 2
```

Normalize with the Rust `xtask` normalizer if available:

```sh
cargo run --release -p xtask -- normalize-nasa-power   --raw-dir experiments/ml-weather/runs/global_grid_7056/raw/nasa_power_hourly   --out experiments/ml-weather/runs/global_grid_7056/normalized/nasa_power_hourly.csv.gz   --workers 16   --pigz-threads 1
```

Then copy or symlink the normalized file to the RTX training path if needed:

```sh
mkdir -p ~/pv-estimator-gpu/data
cp experiments/ml-weather/runs/global_grid_7056/normalized/nasa_power_hourly.csv.gz   ~/pv-estimator-gpu/data/nasa_power_hourly_global_grid_7056.csv.gz
```

## Validate Rebuilt Models

Run the source ensemble benchmark:

```sh
cd ~/pv-estimator-gpu
.venv/bin/python scripts/infer_source_ensemble.py   --locations config/pvgis_benchmark_locations.csv   --fetch-pvgis   --sarah3-mask config/source_coverage/pvgis_sarah3_empirical_grid_mask.json   --nasa-checkpoint results/full_climate_normals_compressor_holdout_768x8/best_model.pt   --era5-checkpoint results/pvgis_era5_climate_normals_compressor_768x8/best_model.pt   --sarah3-checkpoint results/pvgis_sarah3_climate_normals_compressor_768x8/best_model.pt   --out-csv runs/source_ensemble_validation/pvgis_benchmark_30.csv   --out-json runs/source_ensemble_validation/pvgis_benchmark_30.summary.json   --out-estimate-json runs/source_ensemble_validation/pvgis_benchmark_30.estimates.json
```

Expected current benchmark quality:

```text
30-location global benchmark:
  vs PVGIS-ERA5:   MAE about 3.47%
  vs PVGIS-SARAH3: MAE about 3.97%

50-city Italy benchmark:
  vs PVGIS-ERA5:   MAE about 2.52%
  vs PVGIS-SARAH3: MAE about 2.31%

120-city regional benchmark:
  vs PVGIS-ERA5:   MAE about 2.94%
  vs PVGIS-SARAH3: MAE about 3.68%
```

The exact numbers may move slightly if upstream source APIs change their data,
if a different PyTorch/CUDA version changes deterministic behavior, or if the
random seed changes. The important acceptance target is that the rebuilt ensemble
stays in the same error range and remains better than the single-source models.

## ONNX Runtime Artifacts

Production inference uses ONNX Runtime on CPU. PyTorch remains an experiment and
export dependency only. The default runtime artifacts are dynamically quantized
QInt8 ONNX models using per-tensor weight quantization for `MatMul`/`Gemm` ops.
Do not use per-channel quantization for this model family; the SARAH3 model loses
several percent of annual PV precision with that setting.

After the three `best_model.pt` checkpoints exist, export the runtime artifact
directory with:

```sh
.venv/bin/python scripts/export_source_models_onnx.py \
  --nasa-checkpoint results/full_climate_normals_compressor_holdout_768x8/best_model.pt \
  --era5-checkpoint results/pvgis_era5_climate_normals_compressor_768x8/best_model.pt \
  --sarah3-checkpoint results/pvgis_sarah3_climate_normals_compressor_768x8/best_model.pt \
  --sarah3-mask config/source_coverage/pvgis_sarah3_empirical_grid_mask.json \
  --out-dir artifacts/source-models-768x8-int8
```

The output directory contains:

```text
source-model-artifacts.json
nasa_power.onnx
pvgis_era5.onnx
pvgis_sarah3.onnx
coverage/pvgis_sarah3_empirical_grid_mask.json
```

`source-model-artifacts.json` records source ids, ONNX paths, SHA-256 checksums,
coverage rules, target normalization stats, quantization settings,
PyTorch-vs-FP32-ONNX parity, and FP32-ONNX-vs-INT8 reference deltas. Do not
commit this directory unless a separate artifact policy changes.

The current exported INT8 bundle is about 28 MiB. Each INT8 ONNX model is about
9.65 MiB, roughly 25.3% of the FP32 ONNX size.

Measured FP32 ONNX vs INT8 ONNX PV-output deltas:

```text
Italy 50:
  annual ensemble MAE:   0.054%
  annual max abs error:  0.127%
  monthly ensemble MAE:  0.117%
  monthly p95 abs:       0.385%

Regional 120:
  annual ensemble MAE:   0.092%
  annual max abs error:  0.404%
  monthly ensemble MAE:  0.150%
  monthly p95 abs:       0.422%
```

Run the production CLI against the exported directory:

```sh
cargo run -p pv-cli -- estimate \
  --lat 40.650 \
  --lon 15.643 \
  --location-id it_potenza_user \
  --name Potenza \
  --region Italy \
  --kwp 1 \
  --loss-pct 14 \
  --tilt-deg 30 \
  --azimuth-deg 0 \
  --model-dir artifacts/source-models-768x8-int8 \
  --format table
```

The CLI emits annual/monthly estimates only. The month-hour climate-normal model
outputs are internal inputs to the annual/monthly PV aggregation layer.

## Artifact Policy

Commit:

```text
scripts
small config CSV/JSON files
coverage summaries
benchmark/result markdown
small metrics summaries if useful
```

Do not commit:

```text
raw source JSON
normalized hourly CSV.gz
climate-normal `.npy` tables
PyTorch `.pt` checkpoints
ONNX runtime artifacts unless explicitly published as release artifacts
large benchmark output under runs/
```

If the project wants stronger protection against weight loss, store the three
`best_model.pt` files in an external release artifact or object store. Total
checkpoint size is about 109 MiB for all three float32 models.
