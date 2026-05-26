# Weather MLP 100k GPU Run 1

Date: 2026-05-26

## Dataset

- Source dataset: NASA POWER global grid 408 run
- Normalized input: `experiments/ml-weather/runs/global_grid_408/normalized/nasa_power_hourly.csv.gz`
- Available rows: 17,889,984
- Sampled training rows: 1,000,000
- Sampled validation rows: 100,000
- Validation split: held-out grid locations where `grid_number % 17 == 0`

## Model

- Architecture: 64 encoded inputs -> 256 hidden -> 256 hidden -> 15 outputs
- Parameters: 86,287
- Activation: SiLU
- Outputs: P10/P50/P90 for GHI, DNI, DHI, ambient temperature, wind speed
- Loss: pinball quantile loss
- Epochs: 20
- Batch size: 8,192
- Learning rate: 0.001
- Training stack: PyTorch 2.5.1 + CUDA 12.1
- GPU: NVIDIA GeForce RTX 3090

## Validation Metrics

| Target | MAE | RMSE | P10/P90 coverage |
|---|---:|---:|---:|
| GHI W/m2 | 41.004 | 85.264 | 0.76410 |
| DNI W/m2 | 90.190 | 163.645 | 0.80114 |
| DHI W/m2 | 24.120 | 45.712 | 0.73331 |
| Ambient temperature C | 1.233 | 1.877 | 0.74842 |
| Wind speed m/s | 1.661 | 2.161 | 0.72439 |

- Pinball loss: 9.671913
- Quantile crossing rate before post-sort: 0.060788

## Comparison

| Model | Parameters | Pinball loss | GHI MAE | DNI MAE | DHI MAE | Temp MAE | Wind MAE |
|---|---:|---:|---:|---:|---:|---:|---:|
| Day/hour climatology | n/a | 34.820 | 234.829 | 213.313 | 95.768 | 8.498 | 2.247 |
| MLP 10k run 2 | 9,295 | 29.667 | 200.598 | 170.762 | 85.990 | 7.105 | 2.138 |
| MLP 100k GPU run 1 | 86,287 | 9.672 | 41.004 | 90.190 | 24.120 | 1.233 | 1.661 |

## Notes

The 100k model is a clear improvement over the first 10k model and the simple
climatology baseline on the same sampled train/validation split. Coverage is
near the expected 80% interval for DNI, but low for DHI, temperature, and wind,
so quantile calibration still needs work.

The GPU run showed that training is no longer the bottleneck once tensors are
loaded. CSV parsing and feature encoding took about 33 seconds for the 1M/100k
sample, so the next experiment should cache tensors on the GPU machine before
running larger sweeps.

The model artifact was written on `rtx.homelab` under
`~/pv-estimator-gpu/results/weather_mlp_100k_run1/model.pt` and is not
committed.
