# Source Ensemble Dataset Plan

## Goal

Build a practical multi-source solar/weather dataset for training source-specific
compressor models and deriving production uncertainty bars.

The ensemble should contain:

```text
NASA POWER      global, already collected at 7,056 grid locations
PVGIS-ERA5      global, collected through PVGIS seriescalc
PVGIS-SARAH3    regional satellite source, collected where coverage exists

Parked for now:
Direct NSRDB PSM3 regional satellite source, mainly Americas
```

The goal is not to force all sources to exist everywhere. The goal is to make
coverage explicit and use all available sources at inference time.

## Source Models

Train one compressor per source:

```text
model_nasa_power
model_pvgis_era5
model_pvgis_sarah3
model_pvgis_nsrdb_cancelled
```

Each model should use the same input and output contract:

```text
input: latitude, longitude, month, hour, optional future geography features
output: mean/std GHI, DNI, DHI, ambient temperature, wind speed
```

At prediction time:

```text
always use: NASA POWER + PVGIS-ERA5
add SARAH3 if source coverage exists
NSRDB is parked for now
```

The reported production estimate should use a calibrated center estimate plus
error bars derived from source disagreement, model residuals, year-to-year
variability, and PV conversion uncertainty.

## Location Strategy

Do not use a naive global latitude/longitude grid for PVGIS collection. It wastes
requests and storage on open ocean points and creates many predictable coverage
misses.

Create a dedicated PVGIS source-ensemble location set:

```text
target size: 2,000 locations for the first serious dataset
scope: land and near-coast only
latitude range: roughly -60 to +65 degrees for first pass
sampling: region-balanced, not purely uniform
```

The dataset should include:

```text
global land coverage for ERA5
extra density in Europe, Africa, and covered South America for SARAH3
Mediterranean / Italy oversample for near-term product relevance
coastal and island locations
mountain and desert locations
validation holdout regions not adjacent to training regions
```

The output should be a checked-in CSV:

```text
experiments/ml-weather/config/source_ensemble_locations_2000.csv
```

Expected fields:

```text
location_id,name,latitude,longitude,region,split_hint,land_context
```

## Coverage Discovery

Coverage is discovered by probing PVGIS, not by hardcoded polygons.

For every `(location, source)` pair, store:

```text
downloaded
coverage_miss
transient_failed
rate_limited
```

Coverage misses are normal for SARAH3. They must be cached as
`.error.json` files so the downloader can resume without re-probing known misses.

Coverage summaries should be committed as small result files, for example:

```text
experiments/ml-weather/results/YYYY-MM-DD_source_ensemble_coverage.md
```

## Collection Strategy

Use PVGIS `seriescalc` through:

```text
experiments/ml-weather/scripts/download_pvgis_series.py
```

Default collection window:

```text
2005-2023
```

Default source list:

```text
PVGIS-ERA5,PVGIS-SARAH3
```

Operational settings:

```text
workers: 1 per host
request delay: 2 seconds
jitter: 1 second
timeout: 240 seconds
resume: enabled through raw JSON and .error.json cache files
```

Use the available VPSes for distributed collection:

```text
vps-eu
vps-us
```

Do not rely on `rtx.homelab` for raw collection until disk pressure is fixed.

## Storage Strategy

Raw PVGIS JSON is temporary. It should not be the long-term artifact.

Measured size:

```text
1 location x 1 source x 2005-2023 = about 18 MB raw JSON
1 location x 1 source x 2005-2023 = about 2.6 MB normalized CSV.gz
```

For 2,000 locations, worst-case raw JSON for three PVGIS sources would be too
large:

```text
2,000 x 2 x 18 MB = about 72 GB raw JSON before regional coverage filtering
```

Because SARAH3 is regional, actual raw size should be lower, but the
pipeline must still avoid unbounded raw accumulation.

Use shard compaction:

```text
download shard
normalize shard
build or merge climate-normal accumulators
verify row counts and missing values
delete raw JSON for that shard
keep manifest, normalized shard, and compact normal tables
```

The final monthly-hour climate-normal tables are small enough to keep.

## Pipeline Phases

### Phase 1: Finish 408-Location Pilot

Current state:

```text
408-location global grid split across vps-eu and vps-us
PVGIS-ERA5, PVGIS-SARAH3
2005-2023
```

Tasks:

```text
finish vps-eu shard
pull manifests and normalized CSV summaries
write coverage summary
inspect source coverage by latitude/region
verify normalized fields and row counts
```

Acceptance:

```text
both shards complete or have only documented transient failures
normalized CSVs have zero missing canonical target values for downloaded files
coverage misses are cached and counted
```

### Phase 2: Generate Source-Ensemble Location Set

Create a better training location set than the naive grid.

Tasks:

```text
reuse existing land/terrain data where available
filter open-ocean points
sample 2,000 land/near-coast locations
balance by region and climate type where practical
include holdout regions
write source_ensemble_locations_2000.csv
```

Acceptance:

```text
CSV is deterministic and checked in
locations are visibly land/near-coast
regions and split hints are populated
```

### Phase 3: Coverage Probe

Run a cheap coverage probe over the 2,000-location set for:

```text
PVGIS-ERA5 through PVGIS seriescalc
PVGIS-SARAH3 through PVGIS seriescalc where covered
```

Implementation can use short one-year `seriescalc` requests or the normal
downloader with a small date range.

Acceptance:

```text
coverage summary by source and region
expected coverage: ERA5 near 100%, SARAH3 regional
no unexplained systematic failures
```

### Phase 4: Full PVGIS Collection

Collect `2005-2023` data for the 2,000-location set.

Tasks:

```text
split locations into VPS-sized shards
run one worker per VPS
normalize each shard
delete raw JSON after verification
keep compact normalized CSV.gz and manifests
```

Acceptance:

```text
all shards complete
coverage misses are explicit
normalized data has expected row counts
storage remains below VPS limits
```

### Phase 5: Build Climate-Normal Tables

Build monthly-hour normal tables per source:

```text
pvgis_era5
pvgis_sarah3
pvgis_nsrdb
```

Each table should contain:

```text
mean GHI, DNI, DHI, temperature, wind
std GHI, DNI, DHI, temperature, wind
counts
location keys
metadata
```

Acceptance:

```text
tables load in the existing compressor training scripts or a small adapter
counts are sane for 2005-2023
metadata records source, years, location set, and coverage
```

### Phase 6: Train Source Compressors

Train source-specific compressors with the same architecture family as the NASA
monthly-hour compressor.

Initial architecture:

```text
residual MLP, 768 width, 8 residual blocks
```

Compare against smaller versions if source coverage is limited:

```text
384x6
768x8
```

Acceptance:

```text
per-source holdout metrics saved
model artifacts include source metadata
models can be invoked through a common inference wrapper
```

### Phase 7: Ensemble Calibration

Use source disagreement and validation residuals to calibrate production bars.

Tasks:

```text
run canonical PV system conversion for each source model
compare against PVGIS PVcalc where available
estimate source disagreement distributions by region
estimate model residual distributions by source
define first P10/P50/P90 rule
```

Acceptance:

```text
the system reports a central estimate and uncertainty band
uncertainty band is wider in regions where source disagreement is high
single-source/regional-missing cases are explicitly marked lower confidence
```

## Immediate Next Actions

1. Finish the current 408-location pilot.
2. Pull and summarize the VPS manifests.
3. Generate the land/near-coast 2,000-location set.
4. Run a one-year coverage probe on that set.
5. Decide final shard sizes for full 2005-2023 collection.


## Coverage Improvement Update - 2026-05-31

The 2,000-location one-year PVGIS coverage probe produced source-specific lists:

```text
PVGIS-ERA5 covered locations:      1,972 / 2,000
PVGIS-SARAH3 covered locations:      787 / 2,000
PVGIS-NSRDB through PVGIS:             0 / 2,000, parked for now
Direct NSRDB Americas candidates:    601 / 2,000, parked for now
```

Use these lists for the next full collection instead of probing all sources for
all locations again:

```text
experiments/ml-weather/config/source_coverage/pvgis_era5_locations.csv
experiments/ml-weather/config/source_coverage/pvgis_sarah3_locations.csv
experiments/ml-weather/config/source_coverage/pvgis_sarah3_empirical_boxes.json
experiments/ml-weather/config/source_coverage/pvgis_sarah3_empirical_grid_mask.json
```

The SARAH3 box file is empirical. It is useful as a coarse sampling aid. The
grid-mask file is the deterministic project applicability mask for prediction.
Both are derived from the coverage probe, not from an official SARAH3 polygon.
For new production regions near the boundary, query PVGIS once and cache the
result before expanding the mask.

PVGIS-NSRDB is removed from the PVGIS full-collection path. Direct NSRDB
PSM3 collection is parked for now; if it is resumed later, it is handled by:

```text
experiments/ml-weather/scripts/download_nsrdb_psm3.py
experiments/ml-weather/scripts/normalize_nsrdb_psm3.py
```
