# Source Ensemble Validation

Date: 2026-06-01

This validates the first three-source climate-normal ensemble against PVGIS PVcalc for a 1 kWp crystalline-silicon system at 30 degree tilt, south-facing azimuth, and 14% system losses.

## Inputs

Models:

- NASA POWER: `rtx.homelab:~/pv-estimator-gpu/results/full_climate_normals_compressor_holdout_768x8/best_model.pt`
- PVGIS-ERA5: `rtx.homelab:~/pv-estimator-gpu/results/pvgis_era5_climate_normals_compressor_768x8/best_model.pt`
- PVGIS-SARAH3: `rtx.homelab:~/pv-estimator-gpu/results/pvgis_sarah3_climate_normals_compressor_768x8/best_model.pt`

Inference uses all applicable models. NASA POWER and PVGIS-ERA5 are always used. PVGIS-SARAH3 is used only when `config/source_coverage/pvgis_sarah3_empirical_grid_mask.json` accepts the coordinate.

## Benchmark Summary

30 benchmark locations were evaluated. PVGIS-SARAH3 reference calls succeeded for 22 of them; failures were expected outside the SARAH3 coverage area.

| Reference | Count | MBE | MAE | RMSE |
| --- | ---: | ---: | ---: | ---: |
| PVGIS-ERA5 | 30 | 0.54% | 3.47% | 4.57% |
| PVGIS-SARAH3 | 22 | 3.01% | 3.97% | 4.93% |

Mean ensemble prediction: 1416.7 kWh/year.
Mean source-disagreement spread: 7.11% of ensemble center.

## Italy Two-Point Check

These are the two user-provided PVGIS comparison points.

| Reference | Count | MBE | MAE | RMSE |
| --- | ---: | ---: | ---: | ---: |
| PVGIS-ERA5 | 2 | 0.40% | 0.66% | 0.77% |
| PVGIS-SARAH3 | 2 | 8.12% | 8.12% | 8.13% |

## Largest ERA5 Reference Errors

| Location | Name | Sources | Ensemble kWh | Error vs ERA5 |
| --- | --- | --- | ---: | ---: |
| `za_cape_town` | Cape Town | nasa_power,pvgis_era5 | 1150.8 | 14.38% |
| `vn_hanoi` | Hanoi | nasa_power,pvgis_era5 | 1142.0 | 9.32% |
| `il_tel_aviv` | Tel Aviv | nasa_power,pvgis_era5 | 1828.7 | 7.66% |
| `ke_nairobi` | Nairobi | nasa_power,pvgis_era5,pvgis_sarah3 | 1502.7 | 6.65% |
| `tr_istanbul` | Istanbul | nasa_power,pvgis_era5,pvgis_sarah3 | 1483.9 | 5.22% |
| `kr_seoul` | Seoul | nasa_power,pvgis_era5 | 1345.1 | -5.18% |

## Largest SARAH3 Reference Errors

| Location | Name | Sources | Ensemble kWh | Error vs SARAH3 |
| --- | --- | --- | ---: | ---: |
| `za_cape_town` | Cape Town | nasa_power,pvgis_era5 | 1150.8 | 10.78% |
| `it_tuscany_apennines` | Tuscany Apennines | nasa_power,pvgis_era5,pvgis_sarah3 | 1385.5 | 8.50% |
| `tr_istanbul` | Istanbul | nasa_power,pvgis_era5,pvgis_sarah3 | 1483.9 | 8.47% |
| `it_potenza` | Potenza area | nasa_power,pvgis_era5,pvgis_sarah3 | 1456.6 | 7.73% |
| `gb_london` | London | nasa_power,pvgis_era5,pvgis_sarah3 | 1078.6 | 6.76% |
| `il_tel_aviv` | Tel Aviv | nasa_power,pvgis_era5 | 1828.7 | 6.45% |

## Artifacts

```text
experiments/ml-weather/runs/source_ensemble_validation/pvgis_benchmark_30.csv
experiments/ml-weather/runs/source_ensemble_validation/pvgis_benchmark_30.summary.json
experiments/ml-weather/runs/source_ensemble_validation/pvgis_italy_2.csv
experiments/ml-weather/runs/source_ensemble_validation/pvgis_italy_2.summary.json
```

## Interpretation

The first ensemble is usable enough to proceed into estimator integration. The main residual risks are regional: Cape Town is high versus both references, Hanoi is high versus ERA5, and SARAH3 applicability remains conservative in places such as Tunisia and Greece. The next implementation step should package the model registry, SARAH3 mask, and inference code behind a stable application API.
