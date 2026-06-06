# Model Card: Tight-v1 INT8 Source-Model Ensemble

## Summary

PV Estimator local v1 uses an embedded INT8 ONNX source-model ensemble to
estimate photovoltaic production from coordinates and system parameters. The
embedded artifact directory is `artifacts/source-models-768x8-int8`, which was
replaced with the tight-v1 INT8 bundle after comparison on 2026-06-05. The same
bundle is published publicly on Hugging Face:

- <https://huggingface.co/lelloman/pv-estimator-tight-v1-int8>

The model is intended for local, approximate PV production estimates. It is not
a substitute for site measurements, a bankability study, structural design,
permitting review, or electrical engineering work.

## Runtime Artifacts

| Artifact | Purpose |
| --- | --- |
| `source-model-artifacts.json` | Manifest, normalization stats, source list, coverage rules. |
| `nasa_power.onnx` | NASA POWER climate-normal source model. |
| `pvgis_era5.onnx` | PVGIS ERA5 source model. |
| `pvgis_sarah3.onnx` | PVGIS SARAH3 source model. |
| `coverage/pvgis_sarah3_empirical_grid_mask.json` | Empirical SARAH3 applicability mask. |

Artifact hashes are recorded in
`reproduction/source-models/results/2026-06-05_tight_v1_int8_comparison.md` and in
the Hugging Face repository `SHA256SUMS` file.

## Manifest

| Field | Value |
| --- | --- |
| Schema version | `1` |
| Model family | `monthly-hourly climate-normal residual MLP` |
| Input features | `66` |
| Temporal bins | `288` |
| Output targets | `10` climate-normal targets |

Targets:

- `ghi_mean_w_m2`
- `dni_mean_w_m2`
- `dhi_mean_w_m2`
- `temp_mean_c`
- `wind_mean_m_s`
- `ghi_std_w_m2`
- `dni_std_w_m2`
- `dhi_std_w_m2`
- `temp_std_c`
- `wind_std_m_s`

## Sources and Coverage

| Source ID | Label | Coverage rule |
| --- | --- | --- |
| `nasa_power` | NASA POWER | Global |
| `pvgis_era5` | PVGIS-ERA5 | Global land PVGIS gateway |
| `pvgis_sarah3` | PVGIS-SARAH3 | Empirical grid mask |

At runtime, the estimator uses only applicable sources for the requested
coordinate. The JSON output reports `coverage.applicable_sources` and
`coverage.pvgis_sarah3_applicable`.

## Inputs

User-facing inputs:

- Latitude and longitude in decimal degrees.
- Peak power in kWp.
- System loss percent.
- Tilt in degrees.
- PVGIS-style azimuth in degrees.

Derived model features include encoded geography, temporal bins, solar geometry,
and system orientation terms. The local v1 manifest schema uses 66 input
features.

## Outputs

The estimator returns:

- Annual energy estimate.
- Monthly energy estimates.
- Annual and monthly in-plane irradiation estimates.
- Annual and monthly global horizontal irradiation estimates.
- Source estimates.
- Ensemble bands and uncertainty bands.
- Coverage metadata.

Energy is represented in watt-hours in JSON. Irradiation is represented in
kilowatt-hours per square meter.

## Evaluation Snapshot

The tight-v1 INT8 comparison used the same `pv-cli estimate` runtime path on a
120-location regional benchmark.

| Reference | Count | Tight v1 MBE | Tight v1 MAE | Tight v1 RMSE |
| --- | ---: | ---: | ---: | ---: |
| PVGIS-ERA5 | 120 | -0.442% | 2.470% | 3.201% |
| PVGIS-SARAH3 | 100 | 2.374% | 3.218% | 4.418% |

Distribution snapshot:

| Reference | p90 abs error | max abs error |
| --- | ---: | ---: |
| PVGIS-ERA5 | 4.892% | 11.623% |
| PVGIS-SARAH3 | 6.469% | 19.456% |

Mean source disagreement spread was `4.785%` in the tight-v1 comparison.

See the full report:

- `reproduction/source-models/results/2026-06-05_tight_v1_int8_comparison.md`

## Limitations

- Estimates are climate-normal predictions, not measured site data.
- Local shading, horizon obstructions, soiling, inverter clipping, curtailment,
  snow, roof heat transfer, and detailed module/inverter behavior are not fully
  modeled in local v1.
- Accuracy can degrade in locations or climates underrepresented by training and
  evaluation data.
- Uncertainty bands are derived from source disagreement and a multiplier. They
  are not a fully calibrated probabilistic forecast for every coordinate.
- SARAH3 coverage is constrained by the empirical mask.

## Intended Use

Appropriate uses:

- Early-stage PV estimate exploration.
- Comparing approximate production across coordinates and simple system settings.
- Local CLI/TUI workflows where no hosted service should be required.

Inappropriate uses:

- Final engineering design.
- Financial guarantees.
- Safety-critical or regulatory decisions without professional review.
- Claims of measured production for a specific site.

## Reproducibility

Use the committed artifacts and run:

```sh
cargo build -p pv-cli
./target/debug/pv estimate --lat 40.4168 --lon=-3.7038 --format json
```

To load the same bundle explicitly instead of using embedded bytes:

```sh
./target/debug/pv estimate \
  --lat 40.4168 \
  --lon=-3.7038 \
  --model-dir artifacts/source-models-768x8-int8 \
  --format json
```

Download the external model bundle from Hugging Face when you want to use the
published weights outside a source checkout:

```sh
hf download lelloman/pv-estimator-tight-v1-int8 \
  --local-dir pv-estimator-tight-v1-int8

./target/debug/pv estimate \
  --lat 40.4168 \
  --lon=-3.7038 \
  --model-dir pv-estimator-tight-v1-int8 \
  --format json
```
