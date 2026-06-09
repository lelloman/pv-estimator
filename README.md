# PV Estimator

[![Release](https://img.shields.io/github/v/release/lelloman/pv-estimator?sort=semver)](https://github.com/lelloman/pv-estimator/releases)
[![Model weights](https://img.shields.io/badge/model%20weights-Hugging%20Face-yellow)](https://huggingface.co/lelloman/pv-estimator-tight-v1-int8)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

PV Estimator estimates photovoltaic production from coordinates and simple
system settings. It runs locally, includes an embedded city search catalog, and
uses an embedded source-model bundle by default, so estimates do not require a
hosted service or a Python runtime.

The current release is a local CLI plus terminal UI:

- `pv search`: find cities from the embedded GeoNames catalog.
- `pv estimate`: estimate annual and monthly PV production from coordinates.
- `pv-tui`: interactive terminal UI for searching locations and adjusting system inputs.

The model bundle is also published separately on Hugging Face:

- <https://huggingface.co/lelloman/pv-estimator-tight-v1-int8>

## Install

Install from a checkout of this repository:

```sh
cargo install --path crates/pv-cli
cargo install --path crates/pv-tui
```

The CLI package is named `pv-cli`, but it installs the command as `pv`.

Release builds and source archives are available on GitHub:

- <https://github.com/lelloman/pv-estimator/releases>

## Quick Start

Search for a city, then estimate with its coordinates:

```sh
pv search Milan
pv estimate --lat 45.4642 --lon 9.1900
```

Use JSON output for scripts or downstream tools:

```sh
pv search Milan --limit 5 --format json
pv estimate --lat 40.4168 --lon=-3.7038 --format json
```

Estimate systems with multiple orientations by adding arrays as
`kWp,tilt,azimuth` triples:

```sh
pv estimate --lat 45.4642 --lon 9.1900 \
  --array 1.5,30,0 \
  --array 2.0,20,-90
```

Use an equals sign for negative longitudes, for example `--lon=-3.7038`, so the
value is not parsed as another flag.

Run the terminal UI:

```sh
pv-tui
```

## What It Estimates

PV Estimator returns climate-normal estimates for:

- annual and monthly PV energy
- annual and monthly in-plane irradiation
- annual and monthly global horizontal irradiation
- per-source estimates and ensemble bands
- source coverage metadata

Inputs are intentionally simple: latitude, longitude, system losses, and one or
more PV arrays. Each array is a peak-power, tilt, and PVGIS-style azimuth triple.
Azimuth `0` means south, `-90` east, and `90` west.

Single-array example:

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
```

Multi-array example:

```sh
pv estimate \
  --lat 45.4642 \
  --lon 9.1900 \
  --name Milan \
  --region IT \
  --loss-pct 14 \
  --array 1.5,30,0 \
  --array 2.0,20,-90 \
  --format json
```

## Local Model Bundle

The default estimator embeds `artifacts/source-models-768x8-int8`, a tight-v1
INT8 ONNX source-model ensemble. It includes source models for:

- NASA POWER
- PVGIS ERA5
- PVGIS SARAH3

The runtime uses only the sources applicable to the requested coordinate and
reports the applicable source list in JSON output. The same bundle is mirrored
on Hugging Face with hashes and metadata:

- <https://huggingface.co/lelloman/pv-estimator-tight-v1-int8>

You can also point the CLI at an explicit model directory:

```sh
pv estimate \
  --lat 40.4168 \
  --lon=-3.7038 \
  --model-dir artifacts/source-models-768x8-int8 \
  --format json
```

Reproduction material for the shipped source models lives in
`reproduction/source-models/`. The tight-v1 comparison report is at:

- `reproduction/source-models/results/2026-06-05_tight_v1_int8_comparison.md`

## Documentation

- [CLI reference](docs/CLI.md): commands, options, JSON shapes, and examples.
- [TUI reference](docs/TUI.md): terminal UI fields, key bindings, search, and saved state.
- [Model card](docs/MODEL_CARD.md): model details, evaluation snapshot, intended use, and limitations.
- [Troubleshooting](docs/TROUBLESHOOTING.md): common install, parsing, model loading, search, and TUI issues.
- [Packaging](docs/PACKAGING.md): release checks, source archive requirements, optimized builds, and man pages.
- [Source-model reproduction](reproduction/source-models/README.md): retained rebuild path for the shipped bundle.

## Limitations

PV Estimator is an early-stage estimation tool. It is useful for local
exploration and rough production comparisons, but it is not a site survey, a
bankability report, an electrical design tool, or a replacement for measured
site data.

Known local-v1 limits:

- `pv estimate` accepts coordinates only. Use `pv search`, then pass latitude and longitude.
- Multiple arrays model capacity and orientation only; string layout, MPPT assignment, and inverter clipping still are not modeled in detail.
- Local shading, horizon obstructions, snow, soiling, curtailment, detailed inverter clipping, and detailed module behavior are not fully modeled.
- Accuracy can degrade in regions or climates underrepresented by the source models.
- Uncertainty bands come from source disagreement; they are not a fully calibrated probabilistic forecast.
- `pv-service` and `pv-wasm` are future adapters, not part of the local release.

## Development

Common checks:

```sh
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build -p pv-cli -p pv-tui
target/debug/pv estimate --lat 40.4168 --lon=-3.7038 --format json
```

Workspace layout:

- `crates/pv-cli`: CLI package, installed as `pv`.
- `crates/pv-tui`: interactive terminal UI.
- `crates/pv-model`: model artifact loading and local inference.
- `crates/pv-data`: embedded city catalog and source-model registry metadata.
- `crates/pv-core`: typed units, project/report contracts, and ensemble data structures.
- `crates/pv-service`: future HTTP service adapter.
- `crates/pv-wasm`: future WASM adapter.
- `xtask`: development-time import and maintenance commands.

Generated build outputs, model-reproduction runs, and bulk data should stay out
of git. The committed reproduction tree keeps only the files needed to rebuild,
export, or validate the shipped source-model bundle.

## License

Licensed under either MIT or Apache-2.0, at your option.
