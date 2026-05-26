# Weather MLP 100k GPU Run 3: 8M Balanced Sample

Date: 2026-05-26

## Dataset

- Source dataset: NASA POWER global grid 408 run
- Normalized input: `experiments/ml-weather/runs/global_grid_408/normalized/nasa_power_hourly.csv.gz`
- Available rows: 17,889,984
- Sampled training rows: 8,000,000
- Sampled validation rows: 500,000
- Training stride: 2
- Validation stride: 2
- Validation split: held-out grid locations where `grid_number % 17 == 0`

## Model

- Architecture: 64 encoded inputs -> 256 hidden -> 256 hidden -> 15 outputs
- Parameters: 86,287
- Activation: SiLU
- Outputs: P10/P50/P90 for GHI, DNI, DHI, ambient temperature, wind speed
- Loss: pinball quantile loss
- Epochs: 20
- Batch size: 32,768
- Learning rate: 0.001
- Training stack: PyTorch 2.5.1 + CUDA 12.1
- GPU: NVIDIA GeForce RTX 3090
- Tensor cache: `~/pv-estimator-gpu/cache/samples_train8000000_val500000_ts2_vs2_v1.npz`

## Validation Metrics

| Target | MAE | RMSE | P10/P90 coverage |
|---|---:|---:|---:|
| GHI W/m2 | 38.867 | 82.570 | 0.74208 |
| DNI W/m2 | 91.502 | 167.190 | 0.77231 |
| DHI W/m2 | 23.651 | 47.165 | 0.71288 |
| Ambient temperature C | 2.126 | 4.053 | 0.61471 |
| Wind speed m/s | 2.107 | 3.334 | 0.68685 |

- Pinball loss: 9.625119
- Quantile crossing rate before post-sort: 0.097681

## Comparison

| Model | Train rows | Val rows | Pinball loss | GHI MAE | DNI MAE | DHI MAE | Temp MAE | Wind MAE |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Day/hour climatology | 1,000,000 | 100,000 | 34.820 | 234.829 | 213.313 | 95.768 | 8.498 | 2.247 |
| MLP 10k run 2 | 1,000,000 | 100,000 | 29.667 | 200.598 | 170.762 | 85.990 | 7.105 | 2.138 |
| MLP 100k GPU run 1 | 1,000,000 | 100,000 | 9.672 | 41.004 | 90.190 | 24.120 | 1.233 | 1.661 |
| MLP 100k GPU run 3 | 8,000,000 | 500,000 | 9.625 | 38.867 | 91.502 | 23.651 | 2.126 | 2.107 |

## Notes

This run used a larger and more globally balanced sample than run 1. The solar
irradiance metrics improved slightly for GHI and DHI, while DNI is roughly flat.
Temperature and wind got worse on the larger validation sample, so the current
model should not be treated as generally better across all targets.

A discarded 10M-row attempt used `train_stride = 1`. Because the normalized file
is sorted by location, that filled the training set from the earlier part of the
file and produced a biased training sample. The balanced run uses
`train_stride = 2`, which spreads samples across the full grid.

The larger run confirmed that cached tensors and resident-GPU training make the
GPU path practical. Cache creation took about 112 seconds; the 20-epoch training
run then completed in seconds once tensors were loaded on the RTX 3090.

The model artifact was written on `rtx.homelab` under
`~/pv-estimator-gpu/results/weather_mlp_100k_run3_8m_balanced/model.pt` and is
not committed.
