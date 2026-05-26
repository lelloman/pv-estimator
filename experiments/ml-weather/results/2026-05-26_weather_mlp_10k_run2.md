# Weather MLP 10k Run 2

Date: 2026-05-26

## Dataset

- Source dataset: NASA POWER global grid 408 run
- Normalized input: `experiments/ml-weather/runs/global_grid_408/normalized/nasa_power_hourly.csv.gz`
- Available rows: 17,889,984
- Sampled training rows: 1,000,000
- Sampled validation rows: 100,000
- Validation split: held-out grid locations where `grid_number % 17 == 0`

## Model

- Architecture: 64 encoded inputs -> 64 hidden -> 64 hidden -> 15 outputs
- Parameters: 9,295
- Activation: SiLU
- Outputs: P10/P50/P90 for GHI, DNI, DHI, ambient temperature, wind speed
- Loss: pinball quantile loss
- Epochs: 20
- Batch size: 256
- Learning rate: 0.001

## Validation Metrics

| Target | MAE | RMSE | P10/P90 coverage |
|---|---:|---:|---:|
| GHI W/m2 | 200.598 | 336.848 | 0.76396 |
| DNI W/m2 | 170.762 | 302.892 | 0.77835 |
| DHI W/m2 | 85.990 | 139.562 | 0.77862 |
| Ambient temperature C | 7.105 | 9.455 | 0.79194 |
| Wind speed m/s | 2.138 | 2.718 | 0.81381 |

- Pinball loss: 29.666918
- Quantile crossing rate before post-sort: 0.01616

## Notes

This is the first successful end-to-end training run. The model is intentionally
small and still crude, but the uncertainty coverage is already close to the
nominal 80% P10/P90 interval. The solar-field median errors are still large, so
next runs should compare against simple climatology baselines, improve the
sampling split, and train the 100k-parameter model from `ML_MODEL_PLAN.md`.

The model artifact was written locally under `experiments/ml-weather/runs/` and
is not committed.
