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

- System pane: editable location and system fields.
- Estimate pane: annual estimate, uncertainty band, source coverage, and monthly production table.
- Footer: status messages above the active key bindings.

## Fields

Editable fields:

| Field | Meaning |
| --- | --- |
| `Name` | Display name for the estimate. |
| `Region` | Region or country label. |
| `Latitude` | Decimal latitude. |
| `Longitude` | Decimal longitude. |
| `Loss %` | System loss percent. |
| `Arrays` | One or more array definitions. The main screen shows an `[Edit]` summary plus a parsed table preview. Azimuth uses the PVGIS convention: `0` south, `-90` east, `90` west. |

Editing `Name`, `Region`, `Latitude`, or `Longitude` marks the location as
`custom`. Applying a city search result sets a GeoNames-backed location id.

Example `Arrays` value:

```text
1.5,30,0; 2.0,20,-90; 1.0,10,90
```

The TUI shows the parsed total kWp and per-array summary table below the
`Arrays` field. Press `Enter` on `Arrays`, or click the row, to open a dedicated
arrays editor. Long text fields stay on one line and scroll horizontally with the
cursor. A leading `<` or trailing `>` marks hidden text beyond the visible field
area.

## Key Bindings

Normal mode:

| Key | Action |
| --- | --- |
| `Up` / `Down` | Move between fields. |
| `Tab` / `Shift+Tab` | Move between fields. |
| `Home` / `End` | Jump to first or last field. |
| `Enter` | Edit selected field. On `Name`, open location search mode. On `Arrays`, open the arrays editor. |
| `l` | Open location search mode. |
| `e` | Recompute estimate. |
| `PageUp` / `PageDown` | Scroll the monthly estimate table. |
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
| `Tab` / `Shift+Tab` | Apply value, recompute, and move field selection. |

Arrays mode:

| Key | Action |
| --- | --- |
| `Up` / `Down` | Move between arrays. |
| `Left` / `Right` | Move between `kWp`, `Tilt`, and `Azimuth` cells. |
| `Enter` | Edit the selected cell, or apply a cell edit. |
| `Tab` / `Shift+Tab` | Move between cells. While editing, apply and move. |
| `a` | Add a new default array. |
| `d` / `Delete` | Remove the selected array. |
| `Esc` | Leave arrays mode, or cancel the current cell edit. |

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

## Estimate Table

The monthly estimate table keeps its headings fixed and scrolls month rows on
small terminals. Use `PageUp` / `PageDown`, or the mouse wheel over the Estimate
pane, to move the visible month window.

## Location Search

The location search opens as a separate screen with a search bar and results list.
Open it with `l` from normal mode or by selecting `Name` and pressing `Enter`.
It uses the same embedded GeoNames catalog as `pv search`. Queries shorter than
two characters show no results. Selecting a city updates:

- `Name`
- `Region`
- `Latitude`
- `Longitude`
- internal `location_id`

Press `Esc` or select `[Cancel]` to return to the main screen without changing the
current location.

## State File

`pv-tui` stores a small JSON state file in the OS-specific user config directory
resolved by the `directories` crate. The state includes selected location id,
location query, and field values. If the state file cannot be read or written,
the TUI continues with defaults and reports the issue in the status line.

## Terminal Requirements

`pv-tui` expects an interactive terminal with raw-mode and alternate-screen
support. It is not intended for non-interactive CI logs. Use `pv estimate` for
scripted or machine-readable workflows.
