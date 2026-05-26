# Interpolation Grid 368 Evaluation

Date: 2026-05-26

## Purpose

This evaluation tests interpolation between known grid points. The model was
trained on the regular 408-point global grid. This dataset uses cell-center
points between neighboring grid coordinates, so every evaluation location is
off-grid and lies between surrounding grid points.

This is different from regional extrapolation. It answers: how well does the
model behave at points inside the coordinate space covered by nearby training
locations?

## Dataset

- Source: NASA POWER hourly point API
- Locations: 368 deterministic cell-center interpolation points
- Latitude range: -56.25 to 56.25 degrees
- Longitude range: -165 to 165 degrees
- Date range: 2020-01-01 through 2024-12-31 UTC
- Rows: 16,136,064
- Missing canonical target values: 0
- Local normalized CSV: `experiments/ml-weather/runs/interpolation_grid_368/normalized/nasa_power_hourly.csv.gz`
- Remote normalized CSV: `~/pv-estimator-gpu/data/nasa_power_hourly_interpolation_grid_368.csv.gz`

## Model

- Artifact: `~/pv-estimator-gpu/results/sweep_8m_256x256x128/model.pt`
- Architecture: 64 encoded inputs -> 256 -> 256 -> 128 -> 15 outputs
- Parameters: 117,263
- Training dataset: 8M balanced sample from the original 408-point grid
- Evaluation stack: PyTorch 2.5.1 + CUDA 12.1
- GPU: NVIDIA GeForce RTX 3090
- Evaluation time: 179.8 seconds

## Metrics

| Target | MAE | RMSE | P10/P90 coverage |
|---|---:|---:|---:|
| GHI W/m2 | 38.844 | 83.433 | 0.82934 |
| DNI W/m2 | 89.145 | 166.253 | 0.83782 |
| DHI W/m2 | 22.658 | 45.691 | 0.80895 |
| Ambient temperature C | 2.072 | 3.329 | 0.63146 |
| Wind speed m/s | 1.832 | 2.409 | 0.65423 |

- Pinball loss: 9.296643
- Quantile crossing rate before post-sort: 0.052108

## Comparison

| Evaluation | Rows | Pinball loss | GHI MAE | DNI MAE | DHI MAE | Temp MAE | Wind MAE |
|---|---:|---:|---:|---:|---:|---:|---:|
| 8M grid split, best epoch | 500,000 | 9.393 | 37.994 | 89.604 | 23.138 | 1.938 | 2.111 |
| 368-point interpolation grid | 16,136,064 | 9.297 | 38.844 | 89.145 | 22.658 | 2.072 | 1.832 |

## Interpretation

The model interpolates well at cell-center points between the known grid
locations. Interpolation performance is comparable to, and slightly better than,
the held-out grid validation score by pinball loss.

This does not prove regional extrapolation. The interpolation points are still
surrounded by nearby training-grid coordinates. The next geography test should
hold out whole latitude/longitude blocks or large regions and report error by
distance to the nearest training point.
