# Auxiliary Geography Global Grid 7056 Run

## Purpose

This run adds reusable geography-side inputs for the dense NASA POWER weather
training set. The artifact is keyed by `location_id`, so model experiments can
try different input shapes without re-downloading or re-sampling rasters.

The first feature shape is tabular but spatially aware: each location has point
features plus aggregate features over circular neighborhoods and eight compass
sectors. This captures cases like islands, coasts, inland regions, and mountain
asymmetry better than point-only features such as distance to water.

## Sources

- NOAA ETOPO 2022 60 arc-second bedrock relief GeoTIFF
  - File: `ETOPO_2022_v1_60s_N90W180_bed.tif`
  - Download size: 478,386,633 bytes
  - Purpose: elevation, bathymetry, water/land proxy, directional relief context
- ESA CCI/C3S 2020 300 m land-cover GeoTIFF mirror
  - File: `ESA-CCI-LULC-300m-P1Y-2020-v2.1.1.tif`
  - Download size: 464,149,451 bytes
  - Purpose: land-cover fractions and directional land-cover context

The source manifest is checked in at:

```text
experiments/ml-weather/config/aux_geography_sources.json
```

## Local Artifacts

- Source cache: `experiments/ml-weather/runs/aux_geography/sources`
- Source cache size: 899 MiB
- Feature CSV: `experiments/ml-weather/runs/global_grid_7056/aux_geography/location_features_v1.csv`
- Feature CSV size: 6.7 MiB
- Feature rows: 7,056 locations plus header

Generated files remain ignored by git under `runs/`.

## Feature Shape

The first generated CSV includes:

- point elevation from ETOPO
- point land-cover class from ESA CCI
- radii: 25 km, 100 km, 250 km
- ETOPO elevation mean/std/min/max for each radius
- ETOPO water/land fraction for each radius
- ETOPO sector elevation mean and water fraction for N, NE, E, SE, S, SW, W, NW
- ESA CCI land-cover fractions for tree, shrub, grass, cropland, wetland, built,
  bare, water, snow/ice
- ESA CCI sector fractions for water, built, tree, cropland, bare

This produces a compact location-level table for tabular model experiments, and
keeps the raw rasters available for later raster-patch or CNN experiments.

## Commands

Install the small geospatial dependency set:

```sh
python3 -m venv experiments/ml-weather/.venv-geo
experiments/ml-weather/.venv-geo/bin/pip install -r experiments/ml-weather/requirements-geo.txt
```

Download the source rasters:

```sh
experiments/ml-weather/.venv-geo/bin/python \
  experiments/ml-weather/scripts/collect_aux_geography.py download
```

Build the 7,056-location feature table:

```sh
experiments/ml-weather/.venv-geo/bin/python \
  experiments/ml-weather/scripts/collect_aux_geography.py features \
  --out experiments/ml-weather/runs/global_grid_7056/aux_geography/location_features_v1.csv \
  --progress-every 250
```

## Timing

On the local workstation:

- source download: roughly 0.9 GiB total
- 100-location smoke extraction: about 28 locations/sec
- full 7,056-location extraction: about 4 minutes

## Notes

- ESA CCI/C3S class code `210` is water and `220` is snow/ice.
- The initial tabular feature pass is intentionally coarse and compact. Higher
  resolution sources such as ESA WorldCover 10 m or Copernicus DEM 30 m should
  be added only after this run shows whether geography features improve held-out
  regional validation.
