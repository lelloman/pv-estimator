# Source Ensemble 2,000-Location Coverage Probe - 2026-05-31
## Scope

```text
locations: source_ensemble_locations_2000.csv
source/location jobs: 6,000
sources: PVGIS-ERA5, PVGIS-SARAH3, PVGIS-NSRDB
years requested: 2023 only
hosts: vps-eu, vps-us
```
## Host Results

| Host | Complete | Jobs | Downloaded | Coverage misses |
|---|---:|---:|---:|---:|
| vps-eu | True | 3000 | 1381 | 1619 |
| vps-us | True | 3000 | 1378 | 1622 |

## Coverage By Source

| Source | Downloaded | Coverage misses | Coverage rate |
|---|---:|---:|---:|
| PVGIS-ERA5 | 1972 | 28 | 98.6% |
| PVGIS-NSRDB | 0 | 2000 | 0.0% |
| PVGIS-SARAH3 | 787 | 1213 | 39.4% |

Combined result:

```text
downloaded: 2759
coverage_miss: 3241
coverage rate: 46.0%
```
## Region Coverage

### PVGIS-ERA5

| Region | Downloaded | Coverage misses | Coverage rate |
|---|---:|---:|---:|
| africa_middle_east | 412 | 1 | 99.8% |
| americas | 592 | 9 | 98.5% |
| asia | 560 | 8 | 98.6% |
| europe_mediterranean | 284 | 5 | 98.3% |
| oceania_south_asia | 124 | 5 | 96.1% |

### PVGIS-NSRDB

| Region | Downloaded | Coverage misses | Coverage rate |
|---|---:|---:|---:|
| africa_middle_east | 0 | 413 | 0.0% |
| americas | 0 | 601 | 0.0% |
| asia | 0 | 568 | 0.0% |
| europe_mediterranean | 0 | 289 | 0.0% |
| oceania_south_asia | 0 | 129 | 0.0% |

### PVGIS-SARAH3

| Region | Downloaded | Coverage misses | Coverage rate |
|---|---:|---:|---:|
| africa_middle_east | 412 | 1 | 99.8% |
| americas | 137 | 464 | 22.8% |
| asia | 4 | 564 | 0.7% |
| europe_mediterranean | 234 | 55 | 81.0% |
| oceania_south_asia | 0 | 129 | 0.0% |

## Interpretation

The land/near-coast source-ensemble set fixes the biggest problem from the naive 408 global grid: ERA5 now covers almost all locations. SARAH3 is available for a substantial regional subset. NSRDB still has limited coverage in this PVGIS gateway run and should be treated as regional/conditional.

This probe used only year 2023, so it should be used for coverage planning, not model training. The next step is full 2005-2023 collection with shard compaction and raw JSON deletion after verification.
