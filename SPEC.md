# PV Estimator Specification

## 1. Product Goal

PV Estimator is a PV system estimation and simulation tool.

The core product is not a TUI. The first deliverable is a UI-agnostic Rust
core that can be reused by multiple frontends:

- a future web application compiled through WASM
- a future desktop application
- a Docker-friendly HTTP service
- possible future CLI or TUI tooling
- possible future educational/game-style interface for learning PV system design

The tool must let users create, save, reopen, validate, and simulate PV systems.

## 2. Core Principles

- The Rust simulation core is the source of truth.
- UI layers are adapters around the core, not owners of the domain model.
- Runtime use should work offline for supported embedded data.
- Saved projects are portable files, not only rows in an app database.
- Data catalogs must be extensible without changing the simulation model.
- The v1 simulation timestep is hourly.
- The first implementation must be useful for real residential hybrid systems,
  not only for toy PV production estimates.
- Because this is an open project with uncertain future UI and simulation
  directions, the design must favor explicit extension points and stable
  interfaces over narrowly minimal v1-only structures.
- Abstractions should be created deliberately around likely axes of change:
  data sources, equipment categories, simulation models, validation rules,
  report formats, persistence formats, and UI adapters.

## 3. Users And Workflows

Primary user workflows:

1. Choose or define a location.
2. Create a PV system project.
3. Select commercial equipment from embedded catalogs, or define custom
   equipment when catalog data is missing.
4. Define mounting groups with shared orientation and inclination.
5. Assign panels to mounting groups.
6. Connect panels into strings.
7. Connect strings to inverter MPPT inputs.
8. Define blocking diodes, cables, batteries, BMS, and load profile.
9. Validate the electrical design.
10. Simulate hourly production, consumption, storage, and grid exchange.
11. Inspect reports by hour, day, month, and year.
12. Save and reopen the project as a portable project file.

Future UI workflows:

- draw or visually arrange the system topology
- connect components interactively
- move components around on a canvas
- show validation warnings directly on the visual topology
- provide an educational mode that teaches PV system design

## 4. Domain Terms

Required domain terms:

- `PV system`: a photovoltaic installation, including panels, strings,
  inverters, storage, wiring, loads, and simulation settings.
- `Location`: coordinates and weather-source availability.
- `Mounting group`: a group of panels sharing tilt, azimuth, and mounting
  assumptions.
- `Panel`: a photovoltaic module.
- `String`: panels electrically connected in series.
- `MPPT input`: inverter input receiving one or more strings.
- `Inverter`: string or hybrid inverter.
- `Blocking diode`: diode used to prevent reverse current.
- `Battery`: storage unit or battery module.
- `BMS`: battery management system.
- `Cable segment`: cable run with material, length, section, and electrical
  role.
- `Load profile`: hourly consumption profile.

## 5. Architecture

The project should be organized as a Rust workspace.

Expected crates:

- `pv-core`: pure domain model, validation, and simulation engine.
- `pv-data`: embedded locations, weather data, and equipment catalogs.
- `pv-wasm`: WASM bindings exposing the core through JSON-compatible APIs.
- `pv-service`: stateless HTTP API suitable for Docker deployment.
- `xtask`: development-time import and normalization tools.

The core crate must avoid direct dependencies on:

- terminal UI frameworks
- web frameworks
- desktop frameworks
- databases
- network access

Network access belongs in data import tooling or optional adapters, not in the
core simulation path.

Architectural boundaries should be treated as public design decisions. When an
implementation can reasonably be expected to vary across future data providers,
electrical models, report consumers, or UI surfaces, the first implementation
should introduce a named interface or module boundary instead of baking the
choice directly into application flow.

## 6. Saved Project Files

Saved projects must use portable, versioned project files.

Default file format:

- JSON for v1
- suggested extension: `.pvproj.json`
- explicit schema version field
- stable identifiers for user-created components
- catalog references for embedded equipment
- inline custom equipment definitions where needed

The format must support future schema migrations.

Project files should contain enough information to reopen and simulate the
system even if the UI changes. If a project references catalog equipment, the
file should store both the catalog reference and enough display metadata to
remain understandable.

## 7. Location Data

The binary must embed a set of locations with coordinates.

Initial location scope:

- Italian capitals as the seed dataset.
- The data model must allow adding many more cities later.
- Custom user-defined coordinates should be supported by the project schema,
  even if embedded weather data is unavailable for that custom location.

Location fields:

- stable location id
- display name
- country/region/province metadata
- latitude
- longitude
- elevation when available
- timezone
- available weather data sources

## 8. Weather And Solar Data

Weather and solar data must be embedded for offline runtime use.

Initial sources:

- PVGIS TMY as the default source where available.
- NASA POWER as a second source for comparison and broader coverage.

Data acquisition happens at development/build time through importer tooling.
Runtime simulation should read normalized embedded data, not call external APIs.

Normalized hourly records should support:

- timestamp or hour-of-year index
- global horizontal irradiance
- direct normal irradiance where available
- diffuse horizontal irradiance where available
- ambient temperature
- wind speed where available
- source id
- source metadata
- quality flags

The simulator must allow selecting the weather source when multiple embedded
sources are available for a location.

Known source documentation:

- PVGIS TMY:
  https://joint-research-centre.ec.europa.eu/photovoltaic-geographical-information-system-pvgis/using-pvgis-5/pvgis-5-tools/pvgis-typical-meteorological-year-tmy-generator_en
- NASA POWER hourly API:
  https://power.larc.nasa.gov/docs/services/api/temporal/hourly/

## 9. Equipment Catalogs

The binary must embed a set of commercial equipment catalogs.

Initial catalog scope:

- EU residential photovoltaic products.
- Official datasheets should be used as the source of catalog data.
- Every catalog item must record source URL and extraction date.
- Users must be able to define custom equipment in project files when a product
  is not in the embedded catalog.

Catalog categories:

- photovoltaic panels
- string inverters
- hybrid inverters
- battery modules or integrated battery systems
- BMS devices where not integrated into the battery product
- blocking diodes
- copper and aluminum cable sections
- common protection/device limits when needed for validation

Panel fields should include:

- manufacturer and model
- dimensions
- nominal power
- area
- module efficiency where available
- Vmp
- Imp
- Voc
- Isc
- temperature coefficient of power
- temperature coefficient of Voc
- temperature coefficient of Isc when available
- NOCT or NMOT when available
- maximum system voltage
- maximum series fuse rating where available

Inverter fields should include:

- manufacturer and model
- inverter type
- AC nominal power
- AC max power where available
- MPPT input count
- MPPT voltage range
- startup voltage
- maximum DC voltage
- maximum input current per MPPT
- maximum short-circuit current per MPPT when available
- supported battery voltage range for hybrid inverters
- efficiency curve or weighted efficiency where available

Battery/BMS fields should include:

- manufacturer and model
- nominal voltage
- usable capacity
- total capacity when available
- charge/discharge power limits
- charge/discharge current limits
- min/max SOC
- round-trip efficiency where available
- supported inverter compatibility metadata when available

Suggested seed product families to investigate from official sources:

- Trina Vertex S+ panels
- Jinko Tiger Neo panels
- JA Solar residential panels
- Canadian Solar residential panels
- Huawei SUN2000 residential/hybrid inverters
- Fronius GEN24 Plus inverters
- SMA Sunny Boy / Sunny Tripower products
- BYD Battery-Box Premium HVS/HVM batteries

## 10. Project Domain Model

A PV system project should contain:

- project metadata
- schema version
- location reference or custom coordinates
- selected weather source
- simulation settings
- component instances
- topology connections
- load profile
- report preferences

Component instances:

- panel instances reference a catalog/custom panel
- mounting groups define tilt, azimuth, albedo, and optional notes
- strings contain ordered panel instance ids
- inverter instances expose MPPT inputs
- cable segments connect logical endpoints
- blocking diodes belong to defined electrical paths
- battery/BMS instances connect to compatible inverter or DC bus
- load profiles define expected consumption

Mounting groups and strings are orthogonal:

- mounting groups describe physical placement/orientation
- strings describe electrical series connections
- panels in one string may belong to different mounting groups, though
  validation should warn when this is likely to produce mismatch losses

## 11. Load Profiles

V1 must support:

- built-in residential load templates
- imported hourly CSV consumption profiles

CSV import should normalize data to the simulator's hourly timestep.

Minimum load profile fields:

- hour-of-year or timestamp
- energy consumed in Wh or kWh
- optional label/source metadata

If a profile has missing hours, validation must report this before simulation.

## 12. Simulation Requirements

The v1 simulator uses an hourly timestep.

It must estimate:

- plane-of-array irradiance for each mounting group
- panel cell temperature
- DC panel output
- string voltage and current
- inverter conversion and clipping
- cable voltage drop and resistive losses
- blocking diode voltage drop and losses
- battery charging/discharging
- battery state of charge
- load served directly by PV
- load served by battery
- grid import
- grid export
- unmet load if the selected operating mode allows it

Reports must support:

- hourly values for any day
- daily totals across the year
- monthly totals
- yearly totals
- production-only views
- self-consumption views
- electrical validation views

## 13. Electrical Validation

Validation must run separately from simulation and should also be callable before
simulation.

Required validation checks:

- string open-circuit voltage at low temperature against inverter and component
  limits
- string maximum power voltage at operating temperatures against MPPT range
- string current against inverter MPPT input current
- short-circuit current against device limits where available
- maximum system voltage against panel datasheet limits
- inverter DC/AC sizing warnings
- incompatible battery/inverter voltage ranges
- BMS charge/discharge current limits
- battery SOC operating bounds
- cable voltage drop
- cable current limits where enough data is available
- blocking diode voltage/current/power ratings
- missing or incomplete weather data
- incomplete load profile
- panels in one string with different orientation/inclination

Validation reports must return stable machine-readable codes plus parameters.

## 14. Public API Shape

The core should expose APIs equivalent to:

- load a project from JSON
- save a project to JSON
- normalize and validate a project
- list embedded locations
- list embedded catalog equipment
- list available weather sources for a location
- run validation
- run simulation
- aggregate reports

WASM and HTTP adapters should expose JSON-compatible equivalents.

Suggested HTTP endpoints:

- `GET /api/locations`
- `GET /api/catalogs`
- `POST /api/projects/normalize`
- `POST /api/projects/validate`
- `POST /api/projects/simulate`

The HTTP service should be stateless in v1. Project persistence is handled by
the client through portable project files.

## 15. Non-Goals For The First Implementation

The first implementation will not include:

- a TUI
- a finished web UI
- a finished desktop app
- a visual topology editor
- a game mode
- runtime calls to PVGIS or NASA APIs during normal simulation
- a shared hosted backend
- automatic product datasheet scraping at runtime

These are valid future directions, but the first milestone is the documented
core, data model, validation engine, simulation engine, WASM adapter, and
Docker-friendly service.

