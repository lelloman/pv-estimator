# TUI Reference

`pv-tui` is the interactive terminal UI for PV Estimator. It runs locally and
uses the embedded source-model bundle unless `--model-dir` is provided.

## Install

```sh
cargo install --path crates/pv-tui
```

Run:

```sh
pv-tui
```

## Options

```sh
pv-tui [--model-dir <DIR>] [--manifest <NAME>]
```

| Option | Default | Description |
| --- | --- | --- |
| `--model-dir <DIR>` | embedded | Directory containing source-model artifacts. |
| `--manifest <NAME>` | `source-model-artifacts.json` | Manifest filename inside `--model-dir`. |

## Layout

The TUI has three main areas:

- Summary line: annual estimate, uncertainty band, and status.
- System pane: editable location and system fields plus city search results.
- Estimate pane: source coverage and monthly production table.

## Fields

Editable fields:

| Field | Meaning |
| --- | --- |
| `Name` | Display name for the estimate. |
| `Region` | Region or country label. |
| `Latitude` | Decimal latitude. |
| `Longitude` | Decimal longitude. |
| `kWp` | PV system peak power. |
| `Loss %` | System loss percent. |
| `Tilt deg` | Panel tilt from horizontal. |
| `Azimuth deg` | PVGIS-style azimuth; `0` south, `-90` east, `90` west. |

Editing `Name`, `Region`, `Latitude`, or `Longitude` marks the location as
`custom`. Applying a city search result sets a GeoNames-backed location id.

## Key Bindings

Normal mode:

| Key | Action |
| --- | --- |
| `Up` / `Down` | Move between fields. |
| `Tab` / `Shift+Tab` | Move between fields. |
| `Home` / `End` | Jump to first or last field. |
| `Enter` | Edit selected field. |
| `l` | Open location search mode. |
| `e` | Recompute estimate. |
| `q` | Quit. |
| `Ctrl+C` | Quit. |

Edit mode:

| Key | Action |
| --- | --- |
| Text keys | Insert text at the field cursor. |
| `Left` / `Right` | Move cursor. |
| `Home` / `End` | Move cursor to start or end. |
| `Backspace` / `Delete` | Remove text. |
| `Enter` | Apply value and recompute. |
| `Esc` | Leave edit mode. |
| `Tab` / `Shift+Tab` | Leave edit mode and move field selection. |

Location mode:

| Key | Action |
| --- | --- |
| Text keys | Edit the city search query. |
| `Up` / `Down` | Move through search results. |
| `Tab` | Move to the next search result. |
| `Left` / `Right` | Move query cursor. |
| `Home` / `End` | Move query cursor to start or end. |
| `Backspace` / `Delete` | Remove query text and refresh results. |
| `Enter` | Apply selected city and recompute. |
| `Esc` | Return to normal mode. |

## Location Search

The location search uses the same embedded GeoNames catalog as `pv search`.
Queries shorter than two characters show no results. Selecting a city updates:

- `Name`
- `Region`
- `Latitude`
- `Longitude`
- internal `location_id`

## State File

`pv-tui` stores a small JSON state file in the OS-specific user config directory
resolved by the `directories` crate. The state includes selected location id,
location query, and field values. If the state file cannot be read or written,
the TUI continues with defaults and reports the issue in the status line.

## Terminal Requirements

`pv-tui` expects an interactive terminal with raw-mode and alternate-screen
support. It is not intended for non-interactive CI logs. Use `pv estimate` for
scripted or machine-readable workflows.
