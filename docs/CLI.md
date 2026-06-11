# CLI Reference

`pv` is the command line entry point for PV Estimator. It runs locally and uses
the embedded source-model bundle unless `--model-dir` is provided.

## Install

```sh
cargo install --path crates/pv-cli
```

The package name is `pv-cli`; the installed binary name is `pv`.

## Global Syntax

```sh
pv <COMMAND>
```

Commands:

- `estimate`: estimate photovoltaic production for explicit coordinates.
- `search`: search the embedded GeoNames city catalog.

Output formats:

- `table`: human-readable terminal output.
- `json`: stable machine-readable output.

Errors are written to stderr and return a non-zero exit code.

## `pv estimate`

```sh
pv estimate --lat <LATITUDE> --lon <LONGITUDE> [OPTIONS]
```

Required options:

| Option | Type | Description |
| --- | --- | --- |
| `--lat <LATITUDE>` | number | Latitude in decimal degrees, from `-90` to `90`. |
| `--lon <LONGITUDE>` | number | Longitude in decimal degrees, from `-180` to `180`. |

Optional options:

| Option | Default | Description |
| --- | --- | --- |
| `--location-id <ID>` | `custom` | Identifier copied into JSON output. |
| `--name <NAME>` | `Custom location` | Display name copied into output. |
| `--region <REGION>` | empty | Region/country label copied into output. |
| `--kwp <KWP>` | `1.0` | PV system peak power in kWp. Must be positive. |
| `--loss-pct <PCT>` | `14.0` | System loss percent. Must be at least `0` and less than `100`. |
| `--tilt-deg <DEG>` | `30.0` | Panel tilt in degrees from horizontal, from `0` to `90`. |
| `--azimuth-deg <DEG>` | `0.0` | PVGIS-style azimuth for the default single array. `0` is south, `-90` east, `90` west. |
| `--storage-kwh <KWH>` | none | Optional usable battery storage capacity metadata. Must be positive when set. It does not change PV estimate math. |
| `--array <[NAME,]KWP,TILT,AZIMUTH>` | none | Add one or more arrays, optionally prefixed with a display name. Can be repeated or contain semicolon-separated entries. When present, `--array` entries define the system instead of `--kwp`, `--tilt-deg`, and `--azimuth-deg`. |
| `--model-dir <DIR>` | embedded | Directory containing `source-model-artifacts.json` and ONNX files. |
| `--manifest <NAME>` | `source-model-artifacts.json` | Manifest filename inside `--model-dir`. |
| `--format <table|json>` | `table` | Output format. |

Use an equals sign for negative longitudes so shells and clap do not treat the
value as another option:

```sh
pv estimate --lat 40.4168 --lon=-3.7038 --format json
```

Examples:

```sh
pv estimate --lat 45.4642 --lon 9.1900

pv estimate \
  --lat 45.4642 \
  --lon 9.1900 \
  --name Milan \
  --region IT \
  --kwp 3.5 \
  --loss-pct 14 \
  --tilt-deg 30 \
  --azimuth-deg 0 \
  --format json

pv estimate \
  --lat 45.4642 \
  --lon 9.1900 \
  --array 1.5,30,0 \
  --array "west roof,2.0,20,-90" \
  --format json

pv estimate \
  --lat 45.4642 \
  --lon 9.1900 \
  --array "south roof,1.5,30,0; west roof,2.0,20,-90" \
  --format json
```

### Estimate JSON Shape

The top-level object is stable for local v1:

```json
{
  "schema_version": 1,
  "location": {
    "location_id": "custom",
    "name": "Custom location",
    "region": "",
    "latitude": 40.4168,
    "longitude": -3.7038
  },
  "system": {
    "peak_power_kwp": 1.0,
    "loss_pct": 14.0,
    "tilt_deg": 30.0,
    "aspect_deg": 0.0
  },
  "coverage": {
    "pvgis_sarah3_applicable": true,
    "applicable_sources": ["nasa_power", "pvgis_era5", "pvgis_sarah3"]
  },
  "ensemble_estimate": {},
  "references": {
    "arrays": [
      {"name": "south roof", "peak_power_kwp": 1.0, "tilt_deg": 30.0, "azimuth_deg": 0.0}
    ]
  }
}
```

`system.peak_power_kwp` is the total installed kWp used for the estimate. For
multi-array estimates, `system.tilt_deg` and `system.aspect_deg` are
capacity-weighted display values; the exact submitted arrays are preserved in
`references.arrays` with optional `name`, `peak_power_kwp`, `tilt_deg`, and `azimuth_deg` for each
array. For single-array estimates, `references.arrays` contains one entry that
matches `--kwp`, `--tilt-deg`, and `--azimuth-deg`.

`ensemble_estimate` contains per-source annual/monthly estimates, ensemble mean
bands, and uncertainty bands. Energy values are stored as watt-hours. Irradiation
values are stored as kilowatt-hours per square meter.

## `pv search`

```sh
pv search <QUERY> [--limit <N>] [--format <table|json>]
```

Arguments and options:

| Argument or option | Default | Description |
| --- | --- | --- |
| `<QUERY>` | required | City query. After trimming, it must contain at least 2 characters. |
| `--limit <N>` | `10` | Maximum result count. Must be from `1` to `50`. |
| `--format <table|json>` | `table` | Output format. |

Examples:

```sh
pv search Milan
pv search Madrid --limit 5
pv search "New York" --format json
```

Table columns are `name`, `country`, `latitude`, `longitude`, `population`, and
`match kind`.

JSON output is an array:

```json
[
  {
    "geoname_id": 3173435,
    "display_name": "Milan",
    "country_code": "IT",
    "latitude": 45.46427,
    "longitude": 9.18951,
    "population": 1371498,
    "feature_code": "PPLA",
    "matched_name": "Milan",
    "match_kind": "exact_primary"
  }
]
```

Match kinds:

| Match kind | Meaning |
| --- | --- |
| `exact_primary` | Query exactly matched the primary city name. |
| `exact_alias` | Query exactly matched an alternate city name. |
| `prefix_primary` | Query matched the start of the primary city name. |
| `prefix_alias` | Query matched the start of an alternate city name. |
| `substring_primary` | Query matched inside the primary city name. |
| `substring_alias` | Query matched inside an alternate city name. |
| `fuzzy_primary` | Query fuzzily matched the primary city name. |
| `fuzzy_alias` | Query fuzzily matched an alternate city name. |

## Local Data and Network Behavior

`pv estimate` and `pv search` run locally. The default estimate path embeds the
ONNX source models and coverage mask in the binary. City search uses the embedded
GeoNames catalog built into `pv-data`.

Builds may need access to the GeoNames zip only if the committed local catalog
artifact is unavailable. The normal repository checkout includes the artifact
used by the build script.

## External Model Bundle

The same tight-v1 INT8 source-model bundle embedded by default is published on
Hugging Face:

- <https://huggingface.co/lelloman/pv-estimator-tight-v1-int8>

After downloading it, pass the downloaded directory with `--model-dir`:

```sh
hf download lelloman/pv-estimator-tight-v1-int8 \
  --local-dir pv-estimator-tight-v1-int8

pv estimate \
  --lat 40.4168 \
  --lon=-3.7038 \
  --model-dir pv-estimator-tight-v1-int8 \
  --format json
```
