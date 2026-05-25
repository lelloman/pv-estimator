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

- `crates/pv-core`: domain model, validation, simulation, reports, project schema
- `crates/pv-data`: embedded locations, weather data, and equipment catalogs
- `crates/pv-wasm`: WASM adapter
- `crates/pv-service`: HTTP service adapter
- `xtask`: development-time import and maintenance commands
