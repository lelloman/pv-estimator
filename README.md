# PV Estimator

PV Estimator is a Rust-first PV system estimation and simulation project.

The project is currently in the architecture and workspace setup phase. The
core design documents are:

- `SPEC.md`
- `IMPLEMENTATION_PLAN.md`
- `ARCHITECTURE.md`
- `DEVELOPMENT.md`

## Development Commands

```sh
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Workspace

- `crates/pv-core`: domain model, validation, simulation, reports, project schema, and source-model ensemble contracts
- `crates/pv-data`: embedded locations, weather data, equipment catalogs, and source-model metadata
- `crates/pv-wasm`: WASM adapter
- `crates/pv-service`: HTTP service adapter
- `xtask`: development-time import and maintenance commands

## Source-Model Ensemble

The ML weather spike currently has three trained climate-normal source models: NASA POWER, PVGIS-ERA5, and PVGIS-SARAH3. `pv-core::source_model` defines the typed registry, annual/monthly source estimates, and source-disagreement bands used for error bars. `pv-data::source_model_registry()` exposes the current model metadata.

Checkpoint execution still lives in `experiments/ml-weather/scripts/infer_source_ensemble.py`; production crates do not depend on PyTorch.
