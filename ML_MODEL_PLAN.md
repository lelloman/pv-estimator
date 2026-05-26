# ML Model Architecture Plan

## Purpose

This file defines the first neural-network experiments for the global ML
research spike. It turns the initial model discussion into a concrete, small set
of experiments that can be reviewed before code or data collection expands.

The goal is to test whether compact coordinate/time-based models can compress
large weather and PV-production datasets enough to be useful. These models are
research artifacts first. They do not replace source-backed weather data or the
deterministic PV simulator until the experiment proves they are worth promoting.

## Scope

Run two initial model sizes on the same harmonized training sample:

- a roughly 10k-parameter Fourier-feature MLP
- a roughly 100k-parameter Fourier-feature MLP

Both models should:

- predict from coordinates and time, not city IDs
- target global non-polar locations first
- use the canonical fields and source mappings in `SOURCE_HARMONIZATION.md`
- report uncertainty bands, not only point estimates
- be evaluated against simple baselines before influencing production design

The first implementation should keep training separate from production crates.

## Inputs

The minimal raw input set is:

- latitude
- longitude
- elevation, when available
- sine and cosine of day-of-year
- sine and cosine of hour-of-day

That gives seven raw features for the simplest coordinate/time representation.

The first experiments should expand those raw features into approximately 64
encoded input features using Fourier-style harmonics. The exact encoding can be
finalized during implementation, but it should include:

- latitude harmonics
- longitude harmonics
- seasonal time harmonics
- hour-of-day harmonics
- simple interaction terms if they improve validation metrics

Source identity may be used for source-aware training experiments, but the
production inference path should not require a source ID. The primary model
contract remains coordinate/time input.

## Outputs

### Weather Quantile Model

The first weather model predicts P10/P50/P90 quantiles for five canonical
weather fields:

- GHI
- DNI
- DHI
- ambient temperature
- wind speed

This gives 15 outputs per hourly sample.

The simulator can then derive PV production from predicted weather fields. This
keeps physical PV behavior reusable and makes the model easier to inspect.

### Canonical PV Output Model

A direct PV-production model can be tested as a second direction. It predicts
P10/P50/P90 production for a small set of canonical PV configurations.

The first version should stay narrow:

- standard residential panel assumptions
- a small number of tilt and azimuth combinations
- hourly production first
- daily, monthly, and yearly aggregations derived during evaluation

This model may be less general than the weather model, but it is useful as a
baseline for whether direct production prediction is simpler or more accurate.

## Architectures

### Model A: 10k Fourier MLP

Target shape:

```text
64 encoded inputs -> 64 hidden -> 64 hidden -> 15 outputs
```

Approximate parameter count: 9.2k, depending on the final input encoding and
output-head details.

### Model B: 100k Fourier MLP

Target shape:

```text
64 encoded inputs -> 256 hidden -> 256 hidden -> 15 outputs
```

Approximate parameter count: 86k, depending on the final input encoding and
output-head details.

### Model C: Optional PV Output MLP

Use the same basic architecture family as Model A or Model B, but adapt the
output head to canonical PV-production outputs.

This should only be added after the weather quantile models have a working
training and evaluation pipeline.

## Training Strategy

Use SiLU activations for the first experiments. ReLU can be tested later if
training speed or export compatibility requires it.

Train quantile heads with pinball loss for P10/P50/P90. For the first spike,
handle quantile crossing by sorting output quantiles after prediction and log
the crossing rate as a diagnostic. A monotonic parameterization can be added
later if crossing is frequent enough to matter.

Normalize inputs and targets using stored training statistics. The statistics
must be written with each experiment run so the model artifact is reproducible.

Train Model A and Model B on the same sample, with the same holdouts, so size
comparisons are meaningful. CPU training should be acceptable for these first
models; dataset I/O and source normalization are likely to dominate runtime
before network size does.

## Evaluation

Evaluate on:

- random held-out coordinates
- held-out geographic regions
- held-out years
- overlapping-source regions where source disagreement can be measured

Compare against:

- PVGIS TMY where available
- NASA climatology or averaged hourly profiles
- nearest-neighbor coordinate lookup
- simple interpolated climatology
- clear-sky geometry plus coarse weather correction

Report:

- hourly MAE/RMSE for each weather field
- daily energy error
- monthly energy error
- yearly energy error
- P10/P90 coverage
- quantile calibration
- seasonal bias
- regional holdout performance
- source-disagreement sensitivity
- model size
- CPU inference latency
- WASM-relevant inference latency where practical

## Data Policy

Do not commit bulk training datasets as part of these experiments.

Commit:

- experiment configs
- source manifests
- small raw parser fixtures
- small normalized fixtures
- metrics summaries
- small model artifacts only after explicit review

Generated training datasets should remain local or external until the project
approves a data-size and redistribution policy.

## Success Criteria

The 100k model should be kept only if it clearly improves over the 10k model on
held-out regions, held-out years, or uncertainty calibration.

If neither model beats simple climatology and nearest-neighbor baselines, the
ML-compression direction should be narrowed or abandoned.

If the 100k model performs well but still misses regional structure, the next
experiments can consider:

- grid or tile embeddings
- mixture-of-experts models
- separate clear-sky and weather-correction components
- source-aware training with source-independent inference

No model should move toward production until the deterministic simulator,
source-backed workflows, and evaluation reports remain cleanly separated.
