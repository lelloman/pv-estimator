# Climate Normals Compressor Results - 2026-05-30

## Context

After raw elevation/terrain patch models failed to beat the coordinate/time baseline,
we tested climate normals as auxiliary inputs. The weather model improved strongly when
monthly-hourly climate-normal features were appended, so the next question was whether
those normals could be compressed into a small neural model instead of shipping the raw
lookup table.

## Full Normal Table

Built from the full normalized NASA POWER 7,056-location hourly CSV:

```text
source: data/nasa_power_hourly_global_grid_7056.csv.gz
rows scanned: 309,391,488
locations: 7,056
month-hour bins: 288
targets: 5
populated bins: 2,032,128
counts per bin: 142-155
raw table size: 86 MB as .npy files
scan time: 96.2 seconds with the Rust streaming builder
```

Outputs per `(location, month, hour)`:

```text
mean GHI, DNI, DHI, temperature, wind
std  GHI, DNI, DHI, temperature, wind
```

## Compressor Models

Input features are latitude/longitude plus month/hour harmonic encodings. Output is the
10-value climate-normal vector above. Validation holds out every 17th location.

### 384x6 Residual MLP

```text
parameters: 1,809,034
artifact size: 7.25 MB
best epoch: 78
mean MAE across 10 outputs: 5.3194
```

Best per-output MAE:

```text
GHI mean:   11.36 W/m2
DNI mean:   17.28 W/m2
DHI mean:    6.06 W/m2
Temp mean:   1.00 C
Wind mean:   0.42 m/s

GHI std:     4.89 W/m2
DNI std:     9.26 W/m2
DHI std:     2.53 W/m2
Temp std:    0.20 C
Wind std:    0.18 m/s
```

### 768x8 Residual MLP

```text
parameters: 9,522,442
artifact size: 38.1 MB
best epoch: 77
mean MAE across 10 outputs: 5.1012
```

Best per-output MAE:

```text
GHI mean:   10.93 W/m2
DNI mean:   16.45 W/m2
DHI mean:    5.93 W/m2
Temp mean:   0.95 C
Wind mean:   0.40 m/s

GHI std:     4.66 W/m2
DNI std:     8.87 W/m2
DHI std:     2.46 W/m2
Temp std:    0.19 C
Wind std:    0.17 m/s
```

## Interpretation

The 768x8 model improves over 384x6, but only modestly for roughly 5x more
parameters. The 384x6 model is likely a better size/quality default unless we tune the
optimizer, use a learning-rate schedule, or try a better architecture.

Climate normals are a much stronger signal than raw elevation/land-cover patch images
for the current weather-prediction task. The compressor can reduce the raw 86 MB normal
table to either about 7 MB or 38 MB, depending on the quality target.

## Artifact Locations On RTX

```text
data/full_climate_normals_7056/
results/full_climate_normals_compressor_holdout_384x6/
results/full_climate_normals_compressor_holdout_768x8/
```

## Next Step

Use compressor-predicted normals as the 10 auxiliary weather-model inputs and compare
against:

```text
coordinate-only baseline
raw-table climate-normal upper bound
compressor-predicted climate normals
```
