# PVGIS 408-Location Coverage Pilot - 2026-05-31
## Scope
```text
locations: 408 global grid points, split across vps-eu and vps-us
sources: PVGIS-ERA5, PVGIS-SARAH3, PVGIS-NSRDB
years: 2005-2023
endpoint: PVGIS seriescalc
```
## Host Results
| Host | Manifest complete | Jobs | Raw statuses | Normalized rows | Missing values | Normalized files |
|---|---:|---:|---|---:|---:|---:|
| vps-eu | True | 612 | `{'coverage_miss': 535, 'skipped_existing': 42, 'downloaded': 35}` | 12,823,272 | 0 | 77 |
| vps-us | True | 612 | `{'coverage_miss': 525, 'downloaded': 87}` | 14,488,632 | 0 | 87 |

## Combined Coverage
| Source | Downloaded/cached | Coverage misses | Total jobs |
|---|---:|---:|---:|
| PVGIS-ERA5 | 121 | 287 | 408 |
| PVGIS-NSRDB | 0 | 408 | 408 |
| PVGIS-SARAH3 | 43 | 365 | 408 |

Combined normalized output:

```text
normalized source/location files: 164
normalized rows: 27,311,904
missing canonical values: 0
```
## Interpretation

The pilot confirms that PVGIS-ERA5 behaves as the global PVGIS source, while SARAH3 and NSRDB are strongly coverage-limited on a naive global grid. Coverage misses are expected and are now cached explicitly as `.error.json` files.

The result also confirms that the normalizer maps downloaded PVGIS files into the canonical schema without missing GHI/DNI/DHI/temperature/wind values for downloaded files.

## Next Step

Use the generated land/near-coast `source_ensemble_locations_2000.csv` for a one-year coverage probe, then run full 2005-2023 collection shard-by-shard with raw JSON deletion after compaction.
