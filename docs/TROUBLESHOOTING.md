# Troubleshooting

This page covers common issues when installing or running PV Estimator locally.

## `--lon` Fails for Negative Values

Use an equals sign for negative longitude values:

```sh
pv estimate --lat 40.4168 --lon=-3.7038
```

Without the equals sign, some argument parsers can interpret `-3.7038` as a new
option instead of the value for `--lon`.

## `pv` Command Not Found After Install

`cargo install` writes binaries to Cargo's bin directory, usually
`$HOME/.cargo/bin`. Add it to `PATH`:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
```

Then reinstall or run:

```sh
cargo install --path crates/pv-cli
pv --help
```

## Installed Binary Is Named `pv`, Not `pv-cli`

This is intentional. The package is `pv-cli`; the installed binary is `pv`.
Use:

```sh
pv estimate --lat 45.4642 --lon 9.1900
```

## ONNX Runtime or Model Loading Errors

The default binary embeds the tight-v1 INT8 ONNX bundle. If `pv estimate` fails
while loading model sessions, check:

- You are running the freshly built `pv` binary, not an older binary on `PATH`.
- The platform is supported by the `ort` crate version pinned in `Cargo.lock`.
- If using `--model-dir`, the directory contains `source-model-artifacts.json`,
  `nasa_power.onnx`, `pvgis_era5.onnx`, `pvgis_sarah3.onnx`, and the SARAH3
  coverage mask at `coverage/pvgis_sarah3_empirical_grid_mask.json`.

To compare embedded and explicit artifact loading:

```sh
pv estimate --lat 40.4168 --lon=-3.7038 --format json
pv estimate \
  --lat 40.4168 \
  --lon=-3.7038 \
  --model-dir artifacts/source-models-768x8-int8 \
  --format json
```

## No Applicable Source Models

Some coordinates may not have any applicable source-model coverage. Check that
latitude and longitude are valid decimal degrees and not swapped. Latitude must
be from `-90` to `90`; longitude must be from `-180` to `180`.

PVGIS SARAH3 is limited by the embedded empirical coverage mask. The estimate
output reports `coverage.applicable_sources` and `coverage.pvgis_sarah3_applicable`.

## City Search Does Not Find the Expected Place

`pv search` uses the embedded GeoNames city catalog. Try:

- Use at least two characters.
- Increase `--limit` up to `50`.
- Search by an alternate spelling or ASCII spelling.
- Use JSON output to inspect `matched_name` and `match_kind`.

Example:

```sh
pv search Milano --limit 20 --format json
```

## TUI Does Not Restore State

`pv-tui` stores a small JSON state file in the user config directory resolved by
the operating system. If state is not restored, the directory may be unavailable
or not writable. The TUI will continue to run with defaults.

## Terminal Display Looks Wrong

`pv-tui` expects an interactive terminal with alternate-screen and raw-mode
support. If rendering looks wrong:

- Try a standard terminal emulator.
- Make sure `TERM` is set correctly.
- Avoid running the TUI inside non-interactive logs or CI jobs.

## Rebuild From a Clean Checkout

```sh
cargo clean
cargo build -p pv-cli -p pv-tui
cargo test --workspace
```

For release checks, also run:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
target/debug/pv estimate --lat 40.4168 --lon=-3.7038 --format json
```
