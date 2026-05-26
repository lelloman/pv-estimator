# NASA POWER Global Grid 408 Run

Date: 2026-05-26

## Scope

- Source: NASA POWER hourly point API
- Source id: `nasa_power_hourly`
- Locations: 408 deterministic non-polar grid points
- Latitude range: -60 to 60 degrees
- Longitude range: -172.5 to 172.5 degrees
- Date range: 2020-01-01 through 2024-12-31 UTC
- Parameters: `ALLSKY_SFC_SW_DWN`, `ALLSKY_SFC_SW_DNI`, `ALLSKY_SFC_SW_DIFF`, `T2M`, `WS2M`

## Local Artifacts

Generated data is ignored by git under `experiments/ml-weather/runs/`.

- Raw JSON directory: `experiments/ml-weather/runs/global_grid_408/raw/nasa_power_hourly/`
- Normalized CSV: `experiments/ml-weather/runs/global_grid_408/normalized/nasa_power_hourly.csv.gz`
- Raw JSON size: 1.64 GB
- Full local run directory size: 1.8 GB
- Normalized CSV.gz size: 219 MB

## Normalization Summary

- Raw files: 408
- Normalized rows: 17,889,984
- Missing canonical target values: 0
- Normalized schema: documented in `experiments/ml-weather/README.md`

## Notes

This dataset is the first trainable backbone dataset, not the final source mix.
NASA POWER is used first because it provides global hourly historical records and
maps directly to the initial weather targets. Additional sources should be added
for validation, bias analysis, and uncertainty calibration rather than blindly
merged into the same target table.
