# ML Weather Experiment

This directory contains research-only tooling for the compact weather/PV model
spike. It is intentionally separate from production crates.

## First Training Set

The first real dataset uses NASA POWER hourly point data because it provides
broad global coverage and the first canonical targets directly:

- `ALLSKY_SFC_SW_DWN` -> `ghi_w_m2`
- `ALLSKY_SFC_SW_DNI` -> `dni_w_m2`
- `ALLSKY_SFC_SW_DIFF` -> `dhi_w_m2`
- `T2M` -> `ambient_temperature_c`
- `WS2M` -> `wind_speed_m_s` with `wind_speed_height_m = 2`

The checked-in location lists are:

- `config/pilot_locations.csv`: a small human-readable smoke set
- `config/global_grid_408_locations.csv`: a deterministic non-polar global grid
- `config/global_grid_368_interpolation_locations.csv`: cell-center points between grid locations
- `config/global_grid_7056_locations.csv`: a denser 2.5-degree non-polar global grid
- `config/global_grid_7056_locations_shuffled.csv`: deterministic shuffled order for resumable large downloads

Generated raw and normalized data goes under `runs/`, which is ignored by git.
Run summaries that are small enough for review go under `results/`.

The current three source models can be rebuilt from the documented process in:

```text
experiments/ml-weather/SOURCE_MODEL_REPRODUCTION.md
```

## Auxiliary Geography

Auxiliary geography data is collected separately from hourly weather records and
keyed by `location_id`. The first generated artifact is a compact spatial
feature table for the 7,056-location grid:

```text
experiments/ml-weather/runs/global_grid_7056/aux_geography/location_features_v1.csv
```

It is generated from global coarse rasters:

- NOAA ETOPO 2022 60 arc-second relief for elevation, bathymetry, and a
  water/land proxy
- ESA CCI/C3S 2020 300 m land cover for land-cover fractions

The derived table contains point features plus aggregate features over 25 km,
100 km, and 250 km radii, including eight compass sectors. This keeps the first
model input shape tabular while still preserving spatial context such as coasts,
islands, and mountain asymmetry.

Install the geospatial experiment dependencies:

```sh
python3 -m venv experiments/ml-weather/.venv-geo
experiments/ml-weather/.venv-geo/bin/pip install -r experiments/ml-weather/requirements-geo.txt
```

Download source rasters and build location features:

```sh
experiments/ml-weather/.venv-geo/bin/python \
  experiments/ml-weather/scripts/collect_aux_geography.py download

experiments/ml-weather/.venv-geo/bin/python \
  experiments/ml-weather/scripts/collect_aux_geography.py features \
  --out experiments/ml-weather/runs/global_grid_7056/aux_geography/location_features_v1.csv \
  --progress-every 250
```

Append auxiliary geography features to an existing reservoir cache without
rescanning the full normalized NASA CSV:

```sh
python3 experiments/ml-weather/scripts/augment_weather_cache_with_aux.py \
  --base-cache experiments/ml-weather/runs/global_grid_7056/cache/samples_reservoir_train16000000_val1000000_ts2_vs2_v2.npz \
  --aux-features experiments/ml-weather/runs/global_grid_7056/aux_geography/location_features_v1.csv \
  --out experiments/ml-weather/runs/global_grid_7056/cache_geo/samples_reservoir_train16000000_val1000000_ts2_vs2_aux87a6a86840e4_v3.npz \
  --train-limit 16000000 \
  --val-limit 1000000 \
  --chunk-size 500000
```

Train the dense residual MLP with auxiliary geography features:

```sh
python3 experiments/ml-weather/scripts/train_weather_mlp_torch.py \
  --data experiments/ml-weather/runs/global_grid_7056/normalized/nasa_power_hourly.csv.gz \
  --out-dir experiments/ml-weather/runs/global_grid_7056/models/weather_residual_geo_384x6 \
  --cache-dir experiments/ml-weather/runs/global_grid_7056/cache_geo \
  --sample-mode reservoir \
  --train-limit 16000000 \
  --val-limit 1000000 \
  --train-stride 2 \
  --val-stride 2 \
  --aux-features experiments/ml-weather/runs/global_grid_7056/aux_geography/location_features_v1.csv \
  --architecture residual-mlp \
  --residual-width 384 \
  --residual-blocks 6 \
  --residual-scale 0.5 \
  --epochs 20 \
  --batch-size 32768 \
  --learning-rate 0.001 \
  --resident-device
```


## PVGIS Source Ensemble Data

PVGIS is used as a data gateway for additional source models, not as a single
source. The first configured PVGIS radiation databases are listed in
`config/pvgis_source_databases.json`:

- `PVGIS-ERA5`: global reanalysis source
- `PVGIS-SARAH3`: satellite-derived source where PVGIS coverage exists

NSRDB is parked for now. The direct downloader/normalizer scaffolding remains in
the repository, but NSRDB is not part of the active source-ensemble collection.

Use the `seriescalc` endpoint for hourly data. The collector requests a
horizontal plane with radiation components (`angle=0`, `components=1`) so the
normalizer can map PVGIS output into the same canonical schema used for NASA:

- `G(i)` -> `ghi_w_m2`
- `Gd(i)` -> `dhi_w_m2`
- `Gb(i) / sin(H_sun)` -> `dni_w_m2`
- `T2m` -> `ambient_temperature_c`
- `WS10m` -> `wind_speed_m_s` with `wind_speed_height_m = 10`

PVGIS publishes a high request-per-second limit, but full hourly downloads are
large. Keep source-model pulls polite and resumable: one worker, a delay, and
per-file caching. Coverage misses such as SARAH3 outside its supported area are
stored as `.error.json` records and do not stop the run.

Smoke download one location/year for ERA5:

```sh
python3 experiments/ml-weather/scripts/download_pvgis_series.py \
  --locations experiments/ml-weather/config/pvgis_benchmark_locations.csv \
  --out-dir experiments/ml-weather/runs/pvgis_series_smoke/raw \
  --databases PVGIS-ERA5 \
  --start-year 2023 \
  --end-year 2023 \
  --limit 1 \
  --workers 1 \
  --request-delay-seconds 2 \
  --request-jitter-seconds 1
```

Normalize downloaded PVGIS JSON into canonical CSV:

```sh
python3 experiments/ml-weather/scripts/normalize_pvgis_series.py \
  --raw-dir experiments/ml-weather/runs/pvgis_series_smoke/raw \
  --out experiments/ml-weather/runs/pvgis_series_smoke/normalized/pvgis_series.csv.gz
```


Generate the first land/near-coast source-ensemble location set from the existing
auxiliary geography table:

```sh
python3 experiments/ml-weather/scripts/generate_source_ensemble_locations.py   --target 2000   --out experiments/ml-weather/config/source_ensemble_locations_2000.csv
```

The generated CSV is intended for PVGIS source coverage probes and later full
source-model collection. It keeps open-ocean points out of the PVGIS workload and
adds `region`, `split_hint`, and `land_context` metadata.

A one-year coverage probe can use the same downloader with `--start-year 2023`
and `--end-year 2023`; this is much smaller than the full 2005-2023 collection
and should be run before the full pull.

A small multi-source run over the benchmark locations can use:

```sh
python3 experiments/ml-weather/scripts/download_pvgis_series.py \
  --locations experiments/ml-weather/config/pvgis_benchmark_locations.csv \
  --out-dir experiments/ml-weather/runs/pvgis_series_benchmark/raw \
  --databases PVGIS-ERA5,PVGIS-SARAH3 \
  --start-year 2005 \
  --end-year 2023 \
  --workers 1 \
  --request-delay-seconds 2 \
  --request-jitter-seconds 1
```


Build source-specific coverage lists from PVGIS coverage manifests:

```sh
python3 experiments/ml-weather/scripts/build_source_coverage_lists.py   --locations experiments/ml-weather/config/source_ensemble_locations_2000.csv   --manifest experiments/ml-weather/runs/source_ensemble_2000_coverage_2023/vps-eu/manifest.json   --manifest experiments/ml-weather/runs/source_ensemble_2000_coverage_2023/vps-us/manifest.json   --out-dir experiments/ml-weather/config/source_coverage
```

The generated lists avoid known PVGIS coverage misses during full collection:

- `source_coverage/pvgis_era5_locations.csv`
- `source_coverage/pvgis_sarah3_locations.csv`

PVGIS-NSRDB did not return coverage in the probe. Direct NSRDB PSM3 support is
parked for now; if it is resumed later, direct NSRDB access requires an API key
and contact metadata:

```sh
export NSRDB_API_KEY=...
export NSRDB_EMAIL=you@example.com
export NSRDB_FULL_NAME="Your Name"
export NSRDB_AFFILIATION="pv-estimator"

python3 experiments/ml-weather/scripts/download_nsrdb_psm3.py   --locations experiments/ml-weather/config/source_coverage/nsrdb_direct_americas_locations.csv   --out-dir experiments/ml-weather/runs/nsrdb_psm3/raw   --start-year 2023   --end-year 2023   --workers 1   --request-delay-seconds 1   --request-jitter-seconds 0.5

python3 experiments/ml-weather/scripts/normalize_nsrdb_psm3.py   --raw-dir experiments/ml-weather/runs/nsrdb_psm3/raw   --out experiments/ml-weather/runs/nsrdb_psm3/normalized/nsrdb_psm3.csv.gz
```

Direct NSRDB API limits are lower than PVGIS for the general API. Keep one worker
and cache every point/year response. The direct CSV endpoint is appropriate for the
first Americas-only source model; a future multipoint POST downloader can reduce
request count if needed.

The SARAH3 coverage probe also generated empirical coverage metadata:

- `source_coverage/pvgis_sarah3_locations.csv`: covered sampled locations
- `source_coverage/pvgis_sarah3_empirical_boxes.json`: coarse region boxes
- `source_coverage/pvgis_sarah3_empirical_grid_mask.json`: row-interval applicability mask

Use the grid mask as the first deterministic applicability check for prediction.
It is project-defined and based on the coverage probe, not an official SARAH3
polygon. For new production regions near the boundary, query PVGIS once and cache
the result before expanding the mask.

Generated PVGIS raw and normalized data stays under `runs/` and is not committed.
Commit only small manifests, summaries, and result notes when they are useful for
review.


## Source Ensemble Inference

The first trained source-model registry is:

```text
experiments/ml-weather/config/source_model_registry.json
```

It records the active source models, external checkpoint paths, coverage rules,
and validation metrics. The current active inference sources are NASA POWER,
PVGIS-ERA5, and PVGIS-SARAH3. SARAH3 must be gated by:

```text
experiments/ml-weather/config/source_coverage/pvgis_sarah3_empirical_grid_mask.json
```

Run the source ensemble on `rtx.homelab`, where the checkpoints live:

```sh
cd ~/pv-estimator-gpu
.venv/bin/python scripts/infer_source_ensemble.py \
  --locations config/pvgis_benchmark_locations.csv \
  --fetch-pvgis \
  --sarah3-mask config/source_coverage/pvgis_sarah3_empirical_grid_mask.json \
  --nasa-checkpoint results/full_climate_normals_compressor_holdout_768x8/best_model.pt \
  --era5-checkpoint results/pvgis_era5_climate_normals_compressor_768x8/best_model.pt \
  --sarah3-checkpoint results/pvgis_sarah3_climate_normals_compressor_768x8/best_model.pt \
  --out-csv runs/source_ensemble_validation/pvgis_benchmark_30.csv \
  --out-json runs/source_ensemble_validation/pvgis_benchmark_30.summary.json
```

The first benchmark result is summarized in:

```text
experiments/ml-weather/results/2026-06-01_source_ensemble_validation.md
```

For a one-off annual/monthly estimate, pass coordinates directly and request the
Rust-compatible estimate JSON payload:

```sh
cd ~/pv-estimator-gpu
.venv/bin/python scripts/infer_source_ensemble.py   --lat 40.650   --lon 15.643   --location-id it_potenza_user   --name Potenza   --region Italy   --fetch-pvgis   --sarah3-mask config/source_coverage/pvgis_sarah3_empirical_grid_mask.json   --nasa-checkpoint results/full_climate_normals_compressor_holdout_768x8/best_model.pt   --era5-checkpoint results/pvgis_era5_climate_normals_compressor_768x8/best_model.pt   --sarah3-checkpoint results/pvgis_sarah3_climate_normals_compressor_768x8/best_model.pt   --out-csv runs/source_ensemble_single/potenza.csv   --out-json runs/source_ensemble_single/potenza.summary.json   --out-estimate-json runs/source_ensemble_single/potenza.estimate.json
```

The `--out-estimate-json` payload uses the `SourceEnsembleEstimateDocument`
shape from `pv-core`, with annual and monthly mean/low/high bands plus the source
models used for the estimate.

## Commands

Download a small smoke sample:

```sh
python3 experiments/ml-weather/scripts/download_nasa_power.py \
  --start 20240101 \
  --end 20240131 \
  --limit 3
```

Download the 408-point global grid set:

```sh
python3 experiments/ml-weather/scripts/download_nasa_power.py \
  --locations experiments/ml-weather/config/global_grid_408_locations.csv \
  --out-dir experiments/ml-weather/runs/global_grid_408/raw/nasa_power_hourly \
  --start 20200101 \
  --end 20241231 \
  --workers 4
```

Generate and download the denser 7,056-point grid. Use the shuffled file for large
pulls so partial checkpoints remain geographically mixed:

```sh
python3 experiments/ml-weather/scripts/generate_global_grid_locations.py \
  --out experiments/ml-weather/config/global_grid_7056_locations.csv \
  --lat-start -60 \
  --lat-stop 60 \
  --lat-step 2.5 \
  --lon-start -178.75 \
  --lon-stop 178.75 \
  --lon-step 2.5

python3 experiments/ml-weather/scripts/shuffle_locations.py \
  --input experiments/ml-weather/config/global_grid_7056_locations.csv \
  --out experiments/ml-weather/config/global_grid_7056_locations_shuffled.csv \
  --seed 7056

python3 experiments/ml-weather/scripts/download_nasa_power.py \
  --locations experiments/ml-weather/config/global_grid_7056_locations_shuffled.csv \
  --out-dir experiments/ml-weather/runs/global_grid_7056/raw/nasa_power_hourly \
  --start 20200101 \
  --end 20241231 \
  --workers 4 \
  --timeout-seconds 60 \
  --retries 2
```

NASA POWER may return HTTP 429 for dense pulls. The downloader writes a partial
manifest and can be resumed with the same command after the rate limit clears.
NASA's multiprocessing tutorial says not to exceed five concurrent requests, so
keep `--workers` totals across machines at or below 5. Use delay and jitter for
long runs:

```sh
python3 experiments/ml-weather/scripts/download_nasa_power.py \
  --locations experiments/ml-weather/config/global_grid_7056_locations_shuffled.csv \
  --out-dir experiments/ml-weather/runs/global_grid_7056/raw/nasa_power_hourly \
  --start 20200101 \
  --end 20241231 \
  --workers 4 \
  --timeout-seconds 60 \
  --retries 2 \
  --request-delay-seconds 1 \
  --request-jitter-seconds 2
```

To split only missing locations into distributed shards:

```sh
python3 experiments/ml-weather/scripts/shard_locations.py \
  --input experiments/ml-weather/config/global_grid_7056_locations_shuffled.csv \
  --out-dir experiments/ml-weather/runs/global_grid_7056/shards \
  --prefix remaining \
  --shards 3 \
  --exclude-raw-dir experiments/ml-weather/runs/global_grid_7056/raw/nasa_power_hourly \
  --start 20200101 \
  --end 20241231
```

Download the smaller pilot set:

```sh
python3 experiments/ml-weather/scripts/download_nasa_power.py \
  --start 20200101 \
  --end 20241231
```

Normalize downloaded JSON into a gzipped CSV. Use the Rust xtask normalizer
for large datasets; it writes parallel gzip shards and concatenates them into a
valid `.csv.gz`:

```sh
cargo run --release -p xtask -- normalize-nasa-power \
  --raw-dir experiments/ml-weather/runs/global_grid_7056/raw/nasa_power_hourly \
  --out experiments/ml-weather/runs/global_grid_7056/normalized/nasa_power_hourly.csv.gz \
  --workers 16 \
  --pigz-threads 1
```

The Python normalizer remains useful for small smoke datasets:

```sh
python3 experiments/ml-weather/scripts/normalize_nasa_power.py \
  --raw-dir experiments/ml-weather/runs/global_grid_408/raw/nasa_power_hourly \
  --out experiments/ml-weather/runs/global_grid_408/normalized/nasa_power_hourly.csv.gz
```

Train the first 10k-parameter weather MLP with the Rust CPU trainer:

```sh
cargo run --release -p xtask -- train-weather-mlp \
  --hidden-width 64 \
  --train-limit 1000000 \
  --val-limit 100000 \
  --epochs 20 \
  --batch-size 256 \
  --learning-rate 0.001 \
  --out-dir experiments/ml-weather/runs/global_grid_408/models/weather_mlp_10k_run2
```

Train the 100k-parameter weather MLP with PyTorch/CUDA on a GPU machine:

```sh
python3 experiments/ml-weather/scripts/train_weather_mlp_torch.py \
  --data experiments/ml-weather/runs/global_grid_408/normalized/nasa_power_hourly.csv.gz \
  --out-dir experiments/ml-weather/runs/global_grid_408/models/weather_mlp_100k_torch \
  --train-limit 1000000 \
  --val-limit 100000 \
  --hidden-layers 256,256 \
  --epochs 20 \
  --batch-size 8192 \
  --learning-rate 0.001
```

For larger GPU runs, cache encoded tensors and keep them resident on the GPU:

```sh
python3 experiments/ml-weather/scripts/train_weather_mlp_torch.py \
  --data experiments/ml-weather/runs/global_grid_408/normalized/nasa_power_hourly.csv.gz \
  --out-dir experiments/ml-weather/runs/global_grid_408/models/weather_mlp_100k_8m \
  --cache-dir experiments/ml-weather/runs/global_grid_408/cache \
  --train-limit 8000000 \
  --val-limit 500000 \
  --train-stride 2 \
  --val-stride 2 \
  --resident-device \
  --hidden-layers 256,256 \
  --epochs 20 \
  --batch-size 32768 \
  --learning-rate 0.001
```

For the 7,056-location dense grid, use reservoir sampling so the training cache
covers the whole sorted CSV instead of stopping after a geographic prefix:

```sh
python3 experiments/ml-weather/scripts/train_weather_mlp_torch.py \
  --data data/nasa_power_hourly_global_grid_7056.csv.gz \
  --out-dir results/dense_7056_16m_256x256x128 \
  --cache-dir cache/dense_7056 \
  --rebuild-cache \
  --sample-mode reservoir \
  --train-limit 16000000 \
  --val-limit 1000000 \
  --train-stride 2 \
  --val-stride 2 \
  --resident-device \
  --hidden-layers 256,256,128 \
  --epochs 20 \
  --batch-size 32768 \
  --learning-rate 0.001
```

Generate cell-center interpolation locations:

```sh
python3 experiments/ml-weather/scripts/generate_interpolation_locations.py
```

Evaluate a trained PyTorch model on a normalized interpolation dataset:

```sh
python3 experiments/ml-weather/scripts/evaluate_weather_mlp_torch.py \
  --data experiments/ml-weather/runs/interpolation_grid_368/normalized/nasa_power_hourly.csv.gz \
  --model experiments/ml-weather/runs/global_grid_408/models/sweep_8m_256x256x128/model.pt \
  --out experiments/ml-weather/runs/interpolation_grid_368/eval/metrics.json
```

Run an architecture sweep on the cached 8M/500k split:

```sh
for spec in 128,128 256,256 512,512 256,256,128; do
  name=$(echo "$spec" | tr , x)
  python3 experiments/ml-weather/scripts/train_weather_mlp_torch.py \
    --data experiments/ml-weather/runs/global_grid_408/normalized/nasa_power_hourly.csv.gz \
    --out-dir experiments/ml-weather/runs/global_grid_408/models/sweep_8m_${name} \
    --cache-dir experiments/ml-weather/runs/global_grid_408/cache \
    --train-limit 8000000 \
    --val-limit 500000 \
    --train-stride 2 \
    --val-stride 2 \
    --resident-device \
    --hidden-layers "$spec" \
    --epochs 20 \
    --batch-size 32768 \
    --learning-rate 0.001
done
```

Compute the day/hour climatology baseline:

```sh
cargo run --release -p xtask -- weather-climatology-baseline \
  --train-limit 1000000 \
  --val-limit 100000 \
  --out-dir experiments/ml-weather/runs/global_grid_408/models/weather_climatology_day_hour
```

## Normalized Schema

The normalized CSV schema is:

```text
source_id,source_record_type,location_id,timestamp_utc,latitude,longitude,
elevation_m,ghi_w_m2,dni_w_m2,dhi_w_m2,ambient_temperature_c,
wind_speed_m_s,wind_speed_height_m,quality_flags
```

## Git Policy

Commit scripts, configs, tiny fixtures, manifests, and metrics summaries.
Do not commit files under `runs/` unless the project explicitly approves a data
artifact policy.
