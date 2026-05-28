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
