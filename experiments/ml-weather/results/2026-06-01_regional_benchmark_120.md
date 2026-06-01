# Regional Source Ensemble Benchmark

Date: 2026-06-01

This benchmark tests the current three-source annual/monthly estimator on 120
cities outside the initial Italy-focused check. The system configuration is 1
kWp crystalline-silicon equivalent, 30 degree tilt, south-facing azimuth, and
14% system losses.

## Location Set

Six 20-city regions:

- Iberia
- France
- Central Europe
- Balkans, Greece, and Turkey
- North Africa and Middle East
- East Asia

Input file:

```text
experiments/ml-weather/config/regional_benchmark_cities_120.csv
```

## Main Result

| Reference | Count | MBE | MAE | RMSE |
| --- | ---: | ---: | ---: | ---: |
| PVGIS-ERA5 | 120 | -0.56% | 2.94% | 4.03% |
| PVGIS-SARAH3 | 100 | 2.16% | 3.68% | 4.95% |

Mean source-disagreement spread: 7.15% of ensemble center.

## Per-Source Result

| Prediction model | vs PVGIS-ERA5 MAE | vs PVGIS-SARAH3 MAE |
| --- | ---: | ---: |
| NASA POWER model | 3.76% | 3.71% |
| PVGIS-ERA5 model | 4.61% | 6.93% |
| PVGIS-SARAH3 model | 4.40% on 83 cities | 3.12% on 83 cities |
| Ensemble | 2.94% | 3.68% |

The ensemble remains better than any single model against ERA5. Against SARAH3,
the SARAH3-only model is best where available, but the ensemble is more broadly
usable because it works outside SARAH3 coverage.

## Regional ERA5 MAE

| Region | Count | MBE | MAE | RMSE | Reference inside source range |
| --- | ---: | ---: | ---: | ---: | ---: |
| Central Europe | 20 | 0.71% | 1.95% | 2.64% | 19/20 |
| North Africa and Middle East | 20 | 0.18% | 2.31% | 3.13% | 13/20 |
| Balkans, Greece, Turkey | 20 | -1.11% | 2.90% | 3.64% | 10/20 |
| Iberia | 20 | -1.60% | 3.08% | 3.89% | 13/20 |
| France | 20 | -2.09% | 3.35% | 4.34% | 10/20 |
| East Asia | 20 | 0.56% | 4.04% | 5.80% | 9/20 |

East Asia is the weakest tested region. It only uses NASA POWER and PVGIS-ERA5
because PVGIS-SARAH3 is not available there.

## SARAH3 Mask Test

The original empirical SARAH3 mask used SARAH3 for 83/120 cities. PVGIS-SARAH3
references existed for 100/120 cities, so the mask is conservative in some
covered areas.

A broad test mask covering Europe, Africa, the Middle East, and nearby Atlantic
islands changed the result to:

| Reference | Original MAE | Broad-mask MAE |
| --- | ---: | ---: |
| PVGIS-ERA5 | 2.94% | 3.06% |
| PVGIS-SARAH3 | 3.68% | 3.64% |

Conclusion: do not blindly expand the SARAH3 mask. It slightly helps SARAH3 but
slightly hurts ERA5. The right fix is a better coverage/applicability model and
possibly source weighting by region.

## Error-Bar Calibration

The raw source range contains the reference about 61% of the time:

| Reference | Inside raw source range |
| --- | ---: |
| PVGIS-ERA5 | 74/120 = 61.7% |
| PVGIS-SARAH3 | 61/100 = 61.0% |

Current raw source half-spread is typically 3.4-3.7% of annual energy. To make a
more conservative displayed range, a preliminary calibration is:

```text
displayed half-range = 2.0 * source half-spread
```

This is roughly an 80th-percentile error band on the current benchmark. For a
more conservative 90th-percentile band, use about 3.0x source half-spread, or a
fixed regional fallback around 7-8% where source disagreement is unrealistically
small.

These are empirical source-spread bands, not formal confidence intervals.

## Worst ERA5 Errors

| Location | Region | Error |
| --- | --- | ---: |
| Taipei | East Asia | 15.23% |
| Chengdu | East Asia | 11.24% |
| Bilbao | Iberia | 10.05% |
| Xian | East Asia | -9.14% |
| Fukuoka | East Asia | 8.47% |
| Marseille | France | -8.21% |
| Toulouse | France | 8.18% |
| Tel Aviv | North Africa/Middle East | 7.66% |

## Artifacts

```text
experiments/ml-weather/runs/source_ensemble_validation/regional_cities_120.csv
experiments/ml-weather/runs/source_ensemble_validation/regional_cities_120.summary.json
experiments/ml-weather/runs/source_ensemble_validation/regional_cities_120.estimates.json
experiments/ml-weather/runs/source_ensemble_validation/regional_cities_120_broad_sarah3.csv
experiments/ml-weather/runs/source_ensemble_validation/regional_cities_120_broad_sarah3.summary.json
```

## Recommendation

Do not fetch a larger global training set yet. The current model is already good
for annual/monthly estimation in Europe and Mediterranean-like regions. The next
higher-value work is:

1. Add calibrated display bands to the estimator output.
2. Improve SARAH3 applicability with more coverage probes and region-aware
   weighting rather than a broad mask.
3. Add targeted East Asia training/evaluation if East Asia is in v1 scope.
