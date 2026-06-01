# PVGIS Benchmark - 30 Locations - 2026-05-30

## Purpose

Test whether the 768x8 climate-normal compressor plus the simple PV conversion layer
produces annual PV estimates that are usable against PVGIS PVcalc references.

This benchmark uses a canonical fixed system:

```text
PV installed: 1 kWp
Technology: crystalline silicon
System loss: 14%
Slope: 30 deg
Azimuth/aspect: 0 deg in PVGIS convention, south-facing
PVGIS endpoint: https://re.jrc.ec.europa.eu/api/v5_3/PVcalc
```

The model side uses:

```text
climate compressor: results/full_climate_normals_compressor_holdout_768x8/best_model.pt
PV conversion: simple PVWatts-style fixed-plane conversion
```

## Coverage

Locations: 30

PVGIS-ERA5 returned data for all 30 locations. PVGIS-SARAH3 returned data for 22
locations; it rejected the India and East/Southeast Asia locations in this benchmark
with HTTP 400, which appears to be a database coverage limitation.

## Summary

Against PVGIS-SARAH3, for the 22 locations with SARAH3 coverage:

```text
energy mean bias error:  +1.44%
energy MAE:               4.64%
energy RMSE:              5.67%
plane-of-array MBE:      -3.27%
plane-of-array MAE:       5.14%
```

Against PVGIS-ERA5, for all 30 locations:

```text
energy mean bias error:  -1.61%
energy MAE:               4.71%
energy RMSE:              6.03%
plane-of-array MBE:      -5.84%
plane-of-array MAE:       6.44%
```

For the 22 locations where both PVGIS databases returned data:

```text
mean SARAH3-vs-ERA5 spread:    2.83% of midpoint
median SARAH3-vs-ERA5 spread:  2.19% of midpoint
model inside database range:   3 / 22 locations
```

## Largest ERA5 Energy Errors

| Location | Model kWh/kWp | ERA5 kWh/kWp | Error |
|---|---:|---:|---:|
| Cape Town | 1150.27 | 1006.11 | +14.33% |
| Delhi | 1393.99 | 1616.59 | -13.77% |
| Riyadh | 1605.51 | 1824.78 | -12.02% |
| Dubai | 1600.19 | 1783.54 | -10.28% |
| Bangkok | 1279.40 | 1414.39 | -9.54% |
| Singapore | 1144.16 | 1254.16 | -8.77% |
| Vienna | 1131.92 | 1214.07 | -6.77% |
| Mumbai | 1441.32 | 1520.46 | -5.20% |

## Largest SARAH3 Energy Errors

| Location | Model kWh/kWp | SARAH3 kWh/kWp | Error |
|---|---:|---:|---:|
| Cape Town | 1150.27 | 1038.81 | +10.73% |
| Riyadh | 1605.51 | 1771.45 | -9.37% |
| Potenza area | 1476.06 | 1351.99 | +9.18% |
| London | 1096.84 | 1010.28 | +8.57% |
| Stockholm | 1050.74 | 969.41 | +8.39% |
| Tuscany Apennines | 1381.35 | 1277.03 | +8.17% |
| Istanbul | 1472.27 | 1367.99 | +7.62% |
| Dubai | 1600.19 | 1710.09 | -6.43% |

## Interpretation

The annual energy MAE is about 4.6-4.7% against either PVGIS database. That is a
credible rough-estimator result for a compressed global model, but not yet a result we
should present as a precise engineering calculator.

The model often does not fall between SARAH3 and ERA5. That matters: database spread
alone is not enough as an error bar. We need calibrated model error bars that combine:

```text
source disagreement
PVGIS year-to-year variability
model residual error by region/climate regime
PV conversion uncertainty
```

## Artifacts

```text
script: experiments/ml-weather/scripts/compare_pvgis_climate_model.py
locations: experiments/ml-weather/config/pvgis_benchmark_locations.csv
remote CSV: ~/pv-estimator-gpu/experiments/ml-weather/runs/pvgis_comparison_30.csv
remote JSON: ~/pv-estimator-gpu/experiments/ml-weather/runs/pvgis_comparison_30.summary.json
local CSV copy: experiments/ml-weather/runs/pvgis_comparison_30.csv
local JSON copy: experiments/ml-weather/runs/pvgis_comparison_30.summary.json
```

## Next Steps

1. Separate weather-model error from PV-conversion error by comparing model
   plane-of-array irradiation directly against PVGIS `H(i)_y`.
2. Add a calibration model for annual kWh/kWp residuals using PVGIS references.
3. Turn the benchmark into a repeatable acceptance gate with region-level metrics.
4. Use calibrated residuals plus PVGIS year-to-year variability to produce P10/P50/P90
   production bars.
