# NASA POWER Global Grid 7056 Partial Pull

Date: 2026-05-27

## Purpose

This pull expands the weather-ML training coverage from the original 408-point
global grid to a denser deterministic non-polar grid. The target grid has 7,056
locations, about 17.3x the original location count.

The download order uses a deterministic shuffled CSV so an interrupted run is
geographically mixed instead of biased toward a latitude-sorted prefix.

## Target Dataset

- Source: NASA POWER hourly point API
- Location config: `experiments/ml-weather/config/global_grid_7056_locations.csv`
- Shuffled config: `experiments/ml-weather/config/global_grid_7056_locations_shuffled.csv`
- Locations: 7,056
- Latitude range: -60 to 60 degrees
- Longitude range: -178.75 to 178.75 degrees
- Grid step: 2.5 degrees latitude and longitude
- Date range: 2020-01-01 through 2024-12-31 UTC
- Full target rows: 309,391,488

## Completed Checkpoint

- Completed locations: 1,395
- Completed rows: 61,167,960
- Missing canonical target values: 0
- Raw JSON size: 5.2G
- Normalized CSV size: 747M
- Local raw path: `experiments/ml-weather/runs/global_grid_7056/raw/nasa_power_hourly`
- Local normalized CSV: `experiments/ml-weather/runs/global_grid_7056/normalized/nasa_power_hourly.csv.gz`
- Normalization summary: `experiments/ml-weather/runs/global_grid_7056/normalized/nasa_power_hourly.csv.gz.summary.json`

## Rate Limit

NASA POWER stopped accepting more requests during this run and returned HTTP
429 from CloudFront. The response did not include a `Retry-After` header; the
body asked the client to try again later or contact the POWER project if rate
limiting persists.

The downloader now records a partial manifest on early stop and reports
`Retry-After` if a future 429 includes it. The checkpoint manifest records all
1,395 completed JSON files already on disk.

## Resume Command

Use a modest worker count when the rate limit clears:

```sh
python3 experiments/ml-weather/scripts/download_nasa_power.py \
  --locations experiments/ml-weather/config/global_grid_7056_locations_shuffled.csv \
  --out-dir experiments/ml-weather/runs/global_grid_7056/raw/nasa_power_hourly \
  --start 20200101 \
  --end 20241231 \
  --workers 4 \
  --timeout-seconds 60 \
  --retries 2
```

Then normalize again to include all newly completed files:

```sh
python3 experiments/ml-weather/scripts/normalize_nasa_power.py \
  --raw-dir experiments/ml-weather/runs/global_grid_7056/raw/nasa_power_hourly \
  --out experiments/ml-weather/runs/global_grid_7056/normalized/nasa_power_hourly.csv.gz
```

## Interpretation

This checkpoint is already a useful larger training source: it is 3.4x the
location count and row count of the 408-point grid, while preserving global
coverage through shuffled download order. It is not the final dense dataset; the
remaining 5,661 locations should be resumed after the NASA rate-limit window
clears.
