# Source Climate-Normal Models 768x8

Date: 2026-06-01

This run assembled the PVGIS source datasets and trained the first PVGIS climate-normal compressor models. The existing NASA POWER 768x8 model is kept as the global baseline/source model.

## Dataset Assembly

| Source | Locations | Hourly rows | Skipped rows | Final CSV |
| --- | ---: | ---: | ---: | --- |
| NASA POWER | 7,056 | 309,391,488 | n/a | `rtx.homelab:~/pv-estimator-gpu/data/nasa_power_hourly_global_grid_7056.csv.gz` |
| PVGIS-ERA5 | 1,972 | 328,408,996 | 4 | `rtx.homelab:~/pv-estimator-gpu/data/pvgis_source_ensemble/pvgis_era5_full_2005_2023.csv.gz` |
| PVGIS-SARAH3 | 787 | 131,063,835 | 3 | `rtx.homelab:~/pv-estimator-gpu/data/pvgis_source_ensemble/pvgis_sarah3_full_2005_2023.csv.gz` |

PVGIS compressed CSVs were assembled from the original VPS shards plus the three-way remaining split across `vps-eu`, `vps-us`, and local.

## Model Results

All three listed models use the residual MLP climate-normal compressor with width 768, 8 residual blocks, and 9,522,442 parameters. Targets are monthly-hourly means and standard deviations for GHI, DNI, DHI, temperature, and wind speed.

| Source | Best epoch | Mean validation MAE | Model path |
| --- | ---: | ---: | --- |
| NASA POWER | 77 | 5.101154 | `rtx.homelab:~/pv-estimator-gpu/results/full_climate_normals_compressor_holdout_768x8/best_model.pt` |
| PVGIS-ERA5 | 75 | 8.333487 | `rtx.homelab:~/pv-estimator-gpu/results/pvgis_era5_climate_normals_compressor_768x8/best_model.pt` |
| PVGIS-SARAH3 | 80 | 7.889470 | `rtx.homelab:~/pv-estimator-gpu/results/pvgis_sarah3_climate_normals_compressor_768x8/best_model.pt` |

## Local Artifacts

Small metadata and metrics files are staged under:

```text
experiments/ml-weather/runs/source_full_2005_2023_assembled/summaries/
experiments/ml-weather/runs/source_full_2005_2023_assembled/training-results/
```

The large raw, normalized, merged CSVs and model checkpoints are not intended to be committed as repository artifacts.
