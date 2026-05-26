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

Download the smaller pilot set:

```sh
python3 experiments/ml-weather/scripts/download_nasa_power.py \
  --start 20200101 \
  --end 20241231
```

Normalize downloaded JSON into a gzipped CSV:

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
  --hidden-width 256 \
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
  --hidden-width 256 \
  --epochs 20 \
  --batch-size 32768 \
  --learning-rate 0.001
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
