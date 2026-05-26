# Source Harmonization Plan

## Purpose

This document defines how weather and solar data sources should be normalized
before they are used for model training, evaluation, or future production data
pipelines.

Multiple sources are useful only if their fields, units, time bases, spatial
meaning, and uncertainty are handled explicitly. They must not be merged as if
they were identical measurements of the same truth.

## Principles

- Preserve source identity in every normalized record.
- Normalize units and names into a canonical schema.
- Mark missing values explicitly.
- Derive missing fields only through documented methods.
- Track whether a value is observed, modeled, reanalysis-derived, satellite
  derived, climatological, typical-year, interpolated, or estimated.
- Treat disagreement between sources as useful uncertainty information.
- Keep raw-source parsers source-specific and keep normalized data
  source-neutral.

## Canonical Record Identity

Every normalized record should identify:

- `source_id`: source and product, such as `nasa_power_hourly`, `pvgis_tmy`, or
  `era5_hourly`.
- `source_version`: dataset/API version when available.
- `source_record_type`: `historical`, `typical_year`, `climatology`,
  `reanalysis`, `satellite`, `forecast`, or `derived`.
- `timestamp_utc` for real historical records, or `hour_of_year` for typical-year
  records.
- `latitude` and `longitude` of the requested point or grid-cell center.
- `elevation` when available.
- `spatial_support`: point estimate, grid-cell estimate, interpolated point, or
  provider-defined value.

## Canonical Weather Fields

Core fields for the first experiments:

| Canonical field | Unit | Required? | Notes |
|---|---:|---:|---|
| `ghi` | W/m2 | yes for solar records | Global horizontal irradiance. |
| `dni` | W/m2 | optional | Direct normal irradiance. |
| `dhi` | W/m2 | optional | Diffuse horizontal irradiance. |
| `ambient_temperature` | deg C | yes when available | Prefer 2 m temperature. |
| `wind_speed` | m/s | optional | Preserve measurement height. |
| `wind_speed_height` | m | optional | Required when wind source height is known. |
| `relative_humidity` | fraction | optional | Normalize percent to 0-1. |
| `surface_pressure` | Pa | optional | Useful for advanced models. |
| `cloud_cover` | fraction | optional | Normalize percent to 0-1. |

Each field should also have metadata when relevant:

- `value_status`: observed, modeled, derived, missing, clipped, quality-filtered.
- `derivation_method`: only set for derived values.
- `quality_flags`: source-specific and canonical flags.

## Source Mapping Policy

### NASA POWER

Initial useful fields:

| NASA field | Canonical field |
|---|---|
| `ALLSKY_SFC_SW_DWN` | `ghi` |
| `ALLSKY_SFC_SW_DNI` | `dni` |
| `ALLSKY_SFC_SW_DIFF` | `dhi` |
| `T2M` | `ambient_temperature` |
| `WS2M` | `wind_speed`, with `wind_speed_height = 2` |

Notes:

- NASA POWER hourly records are historical time-series for requested date
  ranges.
- Missing sentinel values must be converted to missing values, not numeric data.
- NASA can provide broad global coverage and multi-year data, making it a good
  first global source.

### PVGIS TMY

Initial useful fields:

| PVGIS field | Canonical field |
|---|---|
| `G(h)` | `ghi` |
| `Gb(n)` | `dni` |
| `Gd(h)` | `dhi` |
| `T2m` | `ambient_temperature` |
| `WS10m` | `wind_speed`, with `wind_speed_height = 10` |
| `RH` | `relative_humidity` |
| `SP` | `surface_pressure` |

Notes:

- PVGIS TMY is a typical meteorological year, not a contiguous historical year.
- Keep TMY records separate from real historical records during training unless
  the experiment explicitly mixes them.
- TMY is useful as a planning baseline and as a source-backed comparison target.

### ERA5 / Copernicus

Expected useful fields:

| ERA5 concept | Canonical field |
|---|---|
| surface solar radiation downward | `ghi` after time-step conversion |
| 2 m temperature | `ambient_temperature` |
| 10 m u/v wind components | `wind_speed`, derived magnitude, height 10 m |
| surface pressure | `surface_pressure` |
| total cloud cover | `cloud_cover` |

Notes:

- Some ERA5 radiation variables are accumulated energy over a time interval, not
  instantaneous W/m2. Convert carefully to W/m2 using the interval duration.
- ERA5 is gridded reanalysis. Preserve grid-cell identity or interpolation
  method.
- ERA5 is a strong candidate for gridded global sampling, but full global
  multi-decade downloads can be very large.

### NSRDB

Expected useful fields:

| NSRDB field | Canonical field |
|---|---|
| GHI | `ghi` |
| DNI | `dni` |
| DHI | `dhi` |
| Temperature | `ambient_temperature` |
| Wind Speed | `wind_speed` |

Notes:

- Coverage, years, access, and API requirements must be recorded before use.
- NSRDB can be a high-quality validation source where coverage is available.

### SARAH / EUMETSAT

Expected useful fields:

| SARAH concept | Canonical field |
|---|---|
| surface incoming shortwave radiation | `ghi` or related solar field, depending product |
| direct irradiance products where available | `dni` or source-specific solar direct field |

Notes:

- Confirm product-specific variable names, coverage, temporal resolution, and
  licensing before importer work.
- SARAH can be valuable for Europe/Africa/Meteosat-view validation and training.

## Missing-Data Policy

Missing values must be represented explicitly.

Rules:

- Do not encode missing numeric values as zero.
- Convert provider sentinel values to missing values during parsing.
- Preserve a canonical missing flag for every missing optional field.
- Required experiment fields may differ by model target. For example, a GHI-only
  baseline may accept missing DNI/DHI, while a full weather model may not.
- Keep source records even when optional fields are missing if they are useful
  for a lower-dimensional baseline.

## Derivation Policy

Derived fields are allowed only when the method is explicit and testable.

Allowed early derivations:

- wind speed from u/v wind components: `sqrt(u^2 + v^2)`.
- W/m2 from accumulated J/m2 over an interval: divide by interval seconds.
- relative humidity percent to fraction: divide by 100.
- temperature Kelvin to Celsius: subtract 273.15.

Deferred derivations:

- derive DNI/DHI from GHI and solar geometry.
- infer cloud cover from radiation fields.
- convert wind speed between heights.
- bias-correct one source against another.

Deferred derivations need separate validation because they can introduce large
source-specific bias.

## Source Disagreement And Uncertainty

When multiple sources overlap at the same location/time or comparable
location/time:

- compute per-field source spread
- track mean, median, min, max, and robust spread where useful
- record disagreement by region, season, and hour
- use disagreement as an uncertainty feature or evaluation diagnostic
- do not average sources blindly before inspecting bias

For ML experiments, source disagreement can help estimate P10/P90 bounds, but it
should not be treated as ground-truth uncertainty without calibration.

## Training Dataset Shapes

Recommended normalized shapes:

### Long Record Format

One row per source/location/time:

- identity columns
- canonical weather fields
- quality/missing flags
- source metadata references

This is best for parser validation and source-specific modeling.

### Harmonized Feature Format

One row per location/time after source alignment:

- coordinate/time features
- one set of target fields per selected source or aggregation policy
- source availability masks
- disagreement features where overlap exists

This is best for ML training once source compatibility has been analyzed.

## Review Checklist

Before adding a source importer:

- Which canonical fields can this source populate directly?
- Which fields are missing?
- Which units and temporal semantics need conversion?
- Is the value instantaneous, interval average, or accumulated energy?
- Is the location a point, grid cell, or interpolated provider estimate?
- What missing sentinels or quality flags exist?
- What license or terms affect storage and redistribution?
- How will this source be used: training, validation, baseline, or comparison?

