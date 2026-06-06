# Packaging and Distribution

PV Estimator local v1 ships as two Cargo-installable binaries:

- `pv`: command line search and estimate tool from package `pv-cli`.
- `pv-tui`: interactive terminal UI from package `pv-tui`.

The project is not yet shipping the HTTP service, WASM adapter, Debian package,
Docker image, or hosted API as part of local v1.

## Build Prerequisites

- Rust toolchain compatible with the workspace edition (`2024`).
- Cargo.
- Platform support for the `ort` crate pinned in `Cargo.lock`.

The default estimator embeds ONNX model files at compile time from
`artifacts/source-models-768x8-int8`. The same model bundle is published at
<https://huggingface.co/lelloman/pv-estimator-tight-v1-int8> for external
consumers and reproducibility.

## Install From a Checkout

```sh
cargo install --path crates/pv-cli
cargo install --path crates/pv-tui
```

Confirm:

```sh
pv search Milan --limit 1
pv estimate --lat 40.4168 --lon=-3.7038 --format json
pv-tui --help
```

## Build Debug Binaries

```sh
cargo build -p pv-cli -p pv-tui
```

Outputs:

- `target/debug/pv`
- `target/debug/pv-tui`

## Build Optimized Binaries

```sh
cargo build --release -p pv-cli -p pv-tui
```

Outputs:

- `target/release/pv`
- `target/release/pv-tui`

## Release Validation

Run these before publishing binaries or source archives:

```sh
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p pv-cli -p pv-tui
target/release/pv estimate --lat 40.4168 --lon=-3.7038 --format json
target/release/pv search Milan --format json --limit 1
```

## Files Required in Source Archives

Keep these paths in any source distribution:

- `Cargo.toml`
- `Cargo.lock`
- `crates/**`
- `artifacts/source-models-768x8-int8/**`
- `artifacts/geonames/raw/cities1000.zip` or another valid input for the
  `pv-data` build script
- `docs/**`
- `README.md`
- `experiments/ml-weather/results/2026-06-05_tight_v1_int8_comparison.md`

The ONNX files and coverage mask are compile-time inputs. Omitting them breaks
the embedded estimator build. If producing a source archive without embedded
weights, document that users must download the Hugging Face bundle and run with
`--model-dir`; that is not the default local v1 package shape.

## Manual Page

Roff man pages are provided in `docs/man/`. Packagers can install them to a
standard man directory, for example:

```sh
install -Dm644 docs/man/pv.1 /usr/share/man/man1/pv.1
install -Dm644 docs/man/pv-tui.1 /usr/share/man/man1/pv-tui.1
```

## Runtime Network Use

The shipped `pv` and `pv-tui` local v1 binaries do not call a network service for
estimation or city search. They run against embedded model and city catalog data.

## Versioning Notes

The workspace currently uses a single shared Cargo version. If publishing to a
registry or package manager, keep `pv-cli`, `pv-tui`, `pv-model`, `pv-data`, and
`pv-core` versioned together unless a separate compatibility policy is added.

## Future Packaging Work

The following are not implemented in local v1:

- Debian packaging.
- Docker image.
- Homebrew formula.
- Shell completions.
- Service or WASM release artifacts.
