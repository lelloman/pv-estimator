# NASA / PVGIS / Model Source Split - 2026-05-30

## Purpose

Separate three different error sources that were mixed together in the first PVGIS
benchmark:

```text
1. neural compression error: model vs NASA POWER
2. source disagreement: NASA POWER vs PVGIS-SARAH3 / PVGIS-ERA5
3. PV conversion differences: our simple PVWatts-style conversion vs PVGIS PVcalc
```

This run uses the same 30 benchmark locations and the same canonical PV system:

```text
PV installed: 1 kWp
Technology: crystalline silicon for PVGIS
System loss: 14%
Slope: 30 deg
Azimuth/aspect: 0 deg in PVGIS convention, south-facing
NASA period: 2020-01-01 through 2024-12-31
NASA parameters: GHI, DNI, DHI, T2M, WS2M
Model: full_climate_normals_compressor_holdout_768x8/best_model.pt
```

NASA hourly data is converted to annual PV output with the same deterministic PV
conversion layer used for the model. That makes model-vs-NASA mostly a climate-normal
compression/generalization test rather than a PVGIS compatibility test.

## Summary

| Comparison | Count | Mean bias | MAE | RMSE |
|---|---:|---:|---:|---:|
| Model vs NASA | 30 | +2.29% | 3.74% | 4.92% |
| Model POA vs NASA POA | 30 | +1.04% | 3.76% | 4.65% |
| NASA vs PVGIS-SARAH3 | 22 | -0.05% | 4.13% | 5.22% |
| Model vs PVGIS-SARAH3 | 22 | +1.44% | 4.64% | 5.67% |
| NASA vs PVGIS-ERA5 | 30 | -3.69% | 5.36% | 7.23% |
| Model vs PVGIS-ERA5 | 30 | -1.61% | 4.71% | 6.03% |

NASA year-to-year variability across 2020-2024:

```text
mean yearly std:   37.96 kWh/kWp
median yearly std: 34.92 kWh/kWp
```

## Interpretation

The model is much closer to NASA than to either PVGIS database. That is expected: the
model is trained to compress NASA POWER climate behavior.

The first PVGIS benchmark therefore should not be read as pure model error. A large
part of the disagreement is already present between NASA POWER and PVGIS:

```text
NASA vs SARAH3 MAE: 4.13%
NASA vs ERA5 MAE:   5.36%
Model vs NASA MAE:  3.74%
```

The model still has real compression/generalization error. The worst model-vs-NASA
locations are geographically meaningful misses, not random noise.

## Worst Model-vs-NASA Energy Errors

| Location | NASA kWh/kWp | Model kWh/kWp | Error |
|---|---:|---:|---:|
| Beijing | 1341.54 | 1501.50 | +11.92% |
| Istanbul | 1322.65 | 1472.27 | +11.31% |
| Cairo | 1700.01 | 1865.19 | +9.72% |
| Singapore | 1070.09 | 1144.16 | +6.92% |
| Tel Aviv | 1657.12 | 1761.06 | +6.27% |
| Mumbai | 1359.61 | 1441.32 | +6.01% |
| Bangkok | 1359.89 | 1279.40 | -5.92% |
| Tunis | 1474.60 | 1561.24 | +5.88% |

## Worst NASA-vs-ERA5 Source Differences

| Location | NASA kWh/kWp | Model kWh/kWp | NASA vs ERA5 |
|---|---:|---:|---:|
| Delhi | 1318.64 | 1393.99 | -18.43% |
| Singapore | 1070.09 | 1144.16 | -14.68% |
| Cape Town | 1150.64 | 1150.27 | +14.37% |
| Beijing | 1341.54 | 1501.50 | -11.77% |
| Dubai | 1592.82 | 1600.19 | -10.69% |
| Mumbai | 1359.61 | 1441.32 | -10.58% |
| Riyadh | 1652.93 | 1605.51 | -9.42% |
| Seoul | 1297.12 | 1361.25 | -8.56% |

## Consequence For Error Bars

Production bars need multiple components:

```text
P50 estimate: calibrated model output
weather/year variability: NASA yearly std and/or PVGIS SD_y
source uncertainty: NASA-vs-PVGIS disagreement by region/source coverage
model uncertainty: calibrated residuals from model-vs-NASA holdout and PVGIS benchmarks
PV conversion uncertainty: difference between simple conversion and PVGIS loss model
```

A practical first product-level bar can be built as:

```text
sigma_total^2 = sigma_year_to_year^2
              + sigma_model_residual^2
              + sigma_source_disagreement^2
              + sigma_pv_conversion^2
```

Then report something like P50 plus P10/P90 using a calibrated distribution, not just a
single annual kWh number.

## Artifacts

```text
script: experiments/ml-weather/scripts/compare_nasa_pvgis_model.py
locations: experiments/ml-weather/config/pvgis_benchmark_locations.csv
remote CSV: ~/pv-estimator-gpu/experiments/ml-weather/runs/nasa_pvgis_model_comparison_30.csv
remote JSON: ~/pv-estimator-gpu/experiments/ml-weather/runs/nasa_pvgis_model_comparison_30.summary.json
local CSV copy: experiments/ml-weather/runs/nasa_pvgis_model_comparison_30.csv
local JSON copy: experiments/ml-weather/runs/nasa_pvgis_model_comparison_30.summary.json
```
