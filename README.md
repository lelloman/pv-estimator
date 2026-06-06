# PV Estimator

PV Estimator is a local Rust CLI/TUI for photovoltaic production estimates. The
default path runs fully on the local machine with the committed tight-v1 INT8
source-model bundle embedded into the binary.

## Install

From the workspace root:

```sh
cargo install --path crates/pv-cli
cargo install --path crates/pv-tui
```

The CLI package is named `pv-cli`, but it installs the binary as `pv`.

## Documentation

- [CLI reference](docs/CLI.md): full command syntax, options, JSON shapes,
  and examples.
- [TUI reference](docs/TUI.md): terminal UI layout, fields, key bindings, search,
  and state behavior.
- [Troubleshooting](docs/TROUBLESHOOTING.md): common install, argument parsing,
  model loading, city search, and TUI issues.
- [Packaging and distribution](docs/PACKAGING.md): release checks, source archive
  requirements, optimized builds, and man-page installation.
- [Model card](docs/MODEL_CARD.md): embedded tight-v1 INT8 model details,
  evaluation snapshot, intended use, and limitations.
- [Man pages](docs/man/): roff manual pages for packagers.

## CLI Usage

Search for a city, then estimate with coordinates:

```sh
pv search Milan
pv search Milan --limit 5 --format json
pv estimate --lat 45.4642 --lon 9.1900
pv estimate --lat 40.4168 --lon=-3.7038 --format json
```

Negative longitudes should be passed with an equals sign, for example
`--lon=-3.7038`, so the value is not parsed as another flag.

`pv estimate` keeps a stable JSON schema for downstream tools. It accepts
coordinates, system size, loss, tilt, azimuth, optional display fields, and an
optional artifact directory:

```sh
pv estimate \
  --lat 45.4642 \
  --lon 9.1900 \
  --name Milan \
  --region IT \
  --kwp 3.5 \
  --loss-pct 14 \
  --tilt-deg 30 \
  --azimuth-deg 0 \
  --format json

pv estimate \
  --lat 40.4168 \
  --lon=-3.7038 \
  --model-dir artifacts/source-models-768x8-int8 \
  --format json
```

`pv search <query>` returns GeoNames city matches from the embedded catalog.
Table output includes name, country, latitude, longitude, population, and match
kind. JSON output is an array of:

```json
{
  "geoname_id": 3173435,
  "display_name": "Milan",
  "country_code": "IT",
  "latitude": 45.46427,
  "longitude": 9.18951,
  "population": 1371498,
  "feature_code": "PPLA",
  "matched_name": "Milan",
  "match_kind": "exact_primary"
}
```

## TUI Usage

Run the interactive terminal UI:

```sh
pv-tui
```

Use the location search in the TUI to find a city, apply it to the estimate, and
adjust the system fields locally. The TUI stores a small state file in the user
config directory so the last inputs are restored on the next run.

## Model Bundle

The embedded estimator uses `artifacts/source-models-768x8-int8`, which has been
replaced with the tight-v1 INT8 bundle. It contains local ONNX models for NASA
POWER, PVGIS ERA5, and PVGIS SARAH3 plus the SARAH3 coverage mask. The runtime
does not call a network service and does not depend on Python or PyTorch.

For reproducibility, see the tight-v1 comparison report:

- `experiments/ml-weather/results/2026-06-05_tight_v1_int8_comparison.md`

## Known Limitations

- Estimates are climate-normal model predictions, not site measurements or a
  bankability report.
- The CLI does not support `estimate --city` in v1. Use `pv search`, then pass
  coordinates to `pv estimate`.
- Coverage and uncertainty are source-model driven. Some coordinates may have
  fewer applicable sources.
- `pv-service` and `pv-wasm` remain future adapters and are not part of the
  local v1 deliverable.

## Development Checks

```sh
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build -p pv-cli -p pv-tui
target/debug/pv estimate --lat 40.4168 --lon=-3.7038 --format json
```

## Workspace

- `crates/pv-cli`: installed as `pv`; search and estimate commands
- `crates/pv-tui`: local interactive terminal UI
- `crates/pv-model`: embedded/source-model artifact loading and inference
- `crates/pv-data`: embedded GeoNames city catalog and fixture/domain data
- `crates/pv-core`: typed contracts, units, project schema, and reports
- `crates/pv-service`: future HTTP service adapter
- `crates/pv-wasm`: future WASM adapter
- `xtask`: development-time import and maintenance commands
