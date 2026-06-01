# Global ML Weather And PV Research Spike

## Purpose

This research spike tests whether a compact model can approximate solar/weather
behavior and PV production from coordinates and time.

The production roadmap is paused after phase 7 while this experiment is
reviewed. The goal is not to replace the deterministic simulator yet. The goal
is to learn whether a trained model can compress large weather datasets into a
small, useful estimator for arbitrary non-polar locations.

## Research Goal

Train and evaluate models that can estimate:

- monthly climate-normal solar/weather behavior
- annual and monthly PV production behavior
- uncertainty bands, such as P10/P50/P90 estimates
- geographic interpolation for arbitrary coordinates
- variability differences between predictable and less predictable climates

The model should use coordinates rather than city IDs. City data may be used for
sampling, evaluation, and convenience, but the model target is coordinate-based
prediction.

## Source Candidates

Candidate sources to evaluate:

- NASA POWER hourly data for broad global coverage and many historical years.
- PVGIS TMY for typical-year data where coverage is available.
- ERA5/Copernicus reanalysis for global gridded weather variables.
- NSRDB where coverage and API terms make sense.
- EUMETSAT/SARAH where coverage and licensing make sense.
- Solcast only if terms, access, and redistribution constraints are compatible
  with the project.

Each source must have a manifest recording:

- source name
- source documentation URL
- access method
- parameters requested
- coordinate coverage
- time coverage
- license or terms notes
- raw and normalized size estimates
- checksum strategy for downloaded/generated files

## First Questions To Answer

Before bulk downloading or training:

1. How much data can we practically gather from each source?
2. What are the source limits, API constraints, and licensing constraints?
3. What is the smallest useful global sample?
4. Which normalized storage format gives the best size and speed?
5. How much accuracy is lost by using a compact model instead of source-backed
   weather data?
6. How large are useful models likely to be?

## Data Strategy

Start with measurement, not bulk collection.

Initial data work:

- download small samples from each source
- measure raw size
- measure compressed raw size
- normalize into a shared training format
- measure normalized compressed size
- record download time and API constraints
- follow `SOURCE_HARMONIZATION.md` for canonical fields, source mappings,
  missing-data policy, derivation policy, and source-disagreement handling

Training data should be stored in a compact generated format such as
Parquet/Zstd or another columnar/compressed format chosen after measurement.

Git policy:

- commit import code
- commit source manifests
- commit small raw fixtures for parser tests
- commit small normalized fixtures for deterministic tests
- do not commit large raw API responses
- do not commit large generated training datasets until a size policy is
  explicitly approved
- commit small trained model artifacts only when they are useful for review and
  reproducible from the experiment config

## Sampling Strategy

The first global experiment should not attempt every coordinate.

Initial sampling should be stratified by:

- latitude band
- longitude distribution
- elevation where available
- land/coastal context if available
- climate diversity where a simple classification source is available
- solar predictability, including cloudy and clear-sky regions

Holdouts must include:

- random points
- entire geographic regions
- years not used in training
- source-specific holdouts where multiple sources overlap

Extreme polar latitudes can be excluded from the first experiment.

## Model Targets

Train two baseline directions and compare them.

Initial neural-network architectures, feature encoding, output heads, and
training criteria are specified in `ML_MODEL_PLAN.md`.

### Weather Quantile Model

Predict weather-like variables from coordinates and time:

- GHI
- DNI
- DHI
- ambient temperature
- wind speed
- P10/P50/P90 or equivalent uncertainty bands

The simulator can then derive PV production from the predicted weather fields.
This keeps the physical PV model reusable.

### Canonical PV Output Model

Predict PV production quantiles directly for a small set of canonical PV
configurations:

- common tilt/azimuth combinations
- common residential panel/inverter assumptions
- annual and monthly production views first; finer views remain secondary

This may be less general, but it provides a useful baseline for whether direct
production modeling is simpler or more accurate.

## Baselines

The model must beat simple baselines before it can influence production design.

Compare against:

- PVGIS TMY where available
- NASA multi-year climatology or averaged hourly profiles
- nearest-neighbor coordinate lookup
- simple interpolated climatology
- clear-sky geometric model plus coarse weather correction

## Evaluation Metrics

Evaluate at multiple levels:

- monthly energy error
- yearly energy error
- source-disagreement calibration for error bars
- hourly MAE/RMSE for weather fields only as diagnostics
- P10/P90 calibration
- seasonal bias
- regional holdout performance
- source-to-source disagreement
- model size versus dataset size
- inference speed on CPU and WASM-relevant targets where practical

The experiment should explicitly report where the model performs poorly. A
model that works only in predictable climates is still useful if limitations are
clear.

## Implementation Track

Keep this separate from production code.

Suggested structure:

- `experiments/ml-weather/` for research scripts, configs, and notes
- `fixtures/weather/` for tiny checked-in raw and normalized fixtures
- `data/manifests/` for source and sample manifests
- `models/` only for small committed model artifacts approved for review

Production crates should not depend on training frameworks. If a model later
becomes production-worthy, add a small inference boundary rather than coupling
`pv-core` to the training stack.

## Review Steps

Go through the research plan in this order:

1. Confirm the ML goal: global coordinate-based estimates with uncertainty
   bands.
2. Review source candidates and licensing/API constraints.
3. Review `SOURCE_HARMONIZATION.md`: canonical fields, source mappings, missing
   data, derivations, and disagreement policy.
4. Review data volume measurements and storage format choices.
5. Review sampling strategy.
6. Review `ML_MODEL_PLAN.md`: encoded inputs, 10k/100k architectures, output
   heads, losses, and success criteria.
7. Review model targets and baseline definitions.
8. Review evaluation metrics.
9. Decide what data, if any, may be committed.
10. Only then implement experiment scaffolding.

## Non-Goals For The Spike

This spike does not:

- replace the deterministic simulator
- make ML part of production
- require committing bulk weather datasets
- require a web UI
- require exact arbitrary-site certification-grade estimates
- remove PVGIS/NASA/ERA5 source-backed workflows from future consideration

## Success Criteria

The spike is successful if it produces:

- measured data-size estimates for selected sources
- a reproducible small global sample
- at least one weather quantile baseline
- at least one canonical PV output baseline
- evaluation against held-out locations and years
- a clear recommendation: continue, narrow the scope, or abandon ML compression
