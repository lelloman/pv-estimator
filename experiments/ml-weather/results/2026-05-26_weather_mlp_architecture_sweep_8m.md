# Weather MLP Architecture Sweep: 8M Balanced Sample

Date: 2026-05-26

## Dataset

- Source dataset: NASA POWER global grid 408 run
- Sampled training rows: 8,000,000
- Sampled validation rows: 500,000
- Training stride: 2
- Validation stride: 2
- Validation split: held-out grid locations where `grid_number % 17 == 0`
- Tensor cache: `~/pv-estimator-gpu/cache/samples_train8000000_val500000_ts2_vs2_v1.npz`

## Sweep Setup

Common settings:

- Inputs: 64 encoded coordinate/time features
- Outputs: 15 quantile outputs, P10/P50/P90 for five targets
- Activation: SiLU
- Loss: pinball quantile loss
- Epochs: 20
- Batch size: 32,768
- Learning rate: 0.001
- Training stack: PyTorch 2.5.1 + CUDA 12.1
- GPU: NVIDIA GeForce RTX 3090

## Results

| Hidden layers | Parameters | Best epoch | Best pinball | Final pinball | GHI MAE | DNI MAE | DHI MAE | Temp MAE | Wind MAE |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 128,128 | 26,767 | 20 | 9.624942 | 9.624942 | 39.247 | 91.547 | 23.889 | 2.105 | 1.985 |
| 256,256 | 86,287 | 20 | 9.625119 | 9.625119 | 38.867 | 91.502 | 23.651 | 2.126 | 2.107 |
| 512,512 | 303,631 | 20 | 9.751143 | 9.751143 | 40.150 | 92.168 | 24.011 | 2.271 | 2.079 |
| 256,256,128 | 117,263 | 14 | 9.393217 | 9.483358 | 37.994 | 89.604 | 23.138 | 1.938 | 2.111 |

MAE values in the table are from each architecture's best validation-pinball epoch.

## Best Model

The best architecture in this sweep is:

```text
64 inputs -> 256 -> 256 -> 128 -> 15 outputs
```

At epoch 14 it reached:

| Target | MAE | P10/P90 coverage |
|---|---:|---:|
| GHI W/m2 | 37.994 | 0.83158 |
| DNI W/m2 | 89.604 | 0.79316 |
| DHI W/m2 | 23.138 | 0.72340 |
| Ambient temperature C | 1.938 | 0.68064 |
| Wind speed m/s | 2.111 | 0.66505 |

## Interpretation

The small `128,128` model is surprisingly competitive with `256,256` while
using about 31% of the parameters. The wider `512,512` model is worse on this
split, so simply adding width is not helping.

The deeper `256,256,128` model is the first architecture in this set that
clearly beats the previous 100k-family model on validation pinball and most MAE
fields. Its quantile coverage is still weak for DHI, temperature, and wind, so
future work should focus on calibration or target-specific heads rather than
only increasing model size.

The current script saves the final epoch model artifact, not the best-validation
epoch artifact. Future sweeps should add best-checkpoint saving before relying
on model artifacts for downstream evaluation.
