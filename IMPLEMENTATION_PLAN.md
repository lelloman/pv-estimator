# PV Estimator Implementation Plan

## Current Status

- Phase 1: Documentation Baseline - complete.
- Phase 2: Abstraction And Extension Policy - complete.
- Phase 3: Rust Workspace Skeleton - complete.
- Phase 4: Core Units And Types - complete.
- Phase 5: Project Schema - complete.

## 1. Documentation Baseline

Goal: keep the product and engineering target explicit before writing code.

Tasks:

- Maintain `SPEC.md` as the source of product truth.
- Use this file as the ordered implementation checklist.
- Maintain `DEVELOPMENT.md` as the workflow, branching, review, and CI guide.
- Maintain `ARCHITECTURE.md` as the crate boundary and abstraction guide.
- Update the relevant documents when scope, architecture, or process changes.

Acceptance criteria:

- The spec describes the UI-agnostic core, project model, embedded data,
  simulation, validation, extension policy, and deployment targets.
- The development guide describes branching, commits, review rules, CI, and
  compatibility expectations.
- The architecture guide describes extension axes, module boundaries, and core
  contracts.
- No implementation work contradicts the docs without updating them first.

## 2. Abstraction And Extension Policy

Goal: make future growth cheap without losing control of the model.

Tasks:

- Identify the first set of extension axes before implementing core behavior:
  weather sources, equipment catalogs, validation rules, simulation stages,
  report outputs, persistence versions, and UI adapters.
- Define module boundaries and data contracts around those axes even when v1 has
  only one or two implementations.
- Prefer explicit domain types, traits, enums, and versioned data structures over
  primitive maps or ad hoc JSON blobs.
- Document every major abstraction with the future variation it is intended to
  support.

Acceptance criteria:

- Core behavior is not coupled to a specific UI, HTTP service, data source, or
  catalog format.
- Each major abstraction has a named reason tied to likely project evolution.
- The implementation avoids throwaway abstractions that have no plausible second
  implementation or extension path.
- `ARCHITECTURE.md` documents the first set of extension axes and crate
  boundaries before code is introduced.

## 3. Rust Workspace Skeleton

Goal: create the project structure without implementing business logic yet.

Crates:

- `pv-core`
- `pv-data`
- `pv-wasm`
- `pv-service`
- `xtask`

Tasks:

- Initialize a Rust workspace.
- Add minimal crate manifests.
- Configure formatting and lint expectations.
- Add a root README with build/test commands once commands exist.

Acceptance criteria:

- `cargo test --workspace` runs.
- All crates compile with placeholder modules.
- No UI framework is introduced.

## 4. Core Units And Types

Goal: define safe primitives for electrical and energy calculations.

Tasks:

- Add typed wrappers or a unit strategy for voltage, current, power, energy,
  temperature, length, area, angle, and time.
- Define stable IDs for projects, components, locations, catalog items, and
  topology endpoints.
- Define shared error/report code types.

Acceptance criteria:

- Unit conversion tests cover Wh/kWh, W/kW, Celsius/Kelvin where needed,
  degrees/radians, meters, and millimeters squared for cable section.
- Public APIs avoid ambiguous bare numeric values where practical.

## 5. Project Schema

Goal: make saved PV system project files possible before simulation logic.

Tasks:

- Define the versioned `PvSystemProject` model.
- Include metadata, location, weather source, simulation
  settings, component instances, mounting groups, strings, topology, and load
  profile references.
- Implement JSON serialization/deserialization.
- Add schema version handling and a migration placeholder.
- Support catalog references and inline custom equipment definitions.

Acceptance criteria:

- A minimal project round-trips through JSON.
- A representative hybrid project round-trips through JSON.
- Unknown future schema versions return a clear error.
- Missing required fields produce structured validation errors.

## 6. Catalog Model

Goal: represent embedded and custom equipment consistently.

Tasks:

- Define catalog item models for panels, inverters, batteries, BMS, blocking
  diodes, and cable sections.
- Store source URL and extraction date for embedded commercial data.
- Implement lookup by stable catalog id.
- Allow project-local custom equipment with the same simulation fields.

Acceptance criteria:

- Catalog lookup resolves embedded and custom equipment.
- Missing catalog references produce structured validation errors.
- Unit tests cover required fields for each equipment category.

## 7. Location And Weather Data Format

Goal: define normalized embedded data before importers are written.

Tasks:

- Define location records with coordinates, timezone, elevation, and display
  names.
- Seed the location catalog with Italian capitals.
- Define normalized hourly weather records.
- Define weather source metadata for PVGIS and NASA POWER.
- Add small test fixtures before adding full embedded datasets.

Acceptance criteria:

- Locations can be listed from `pv-data`.
- Weather data can be retrieved by location and source id.
- Missing source/location combinations produce clear errors.
- Fixtures are small enough for fast tests.

## 8. Data Import Tooling

Goal: make embedded data reproducible.

Tasks:

- Implement `xtask` commands to import PVGIS TMY data.
- Implement `xtask` commands to import NASA POWER data.
- Normalize imported records to the internal hourly format.
- Write generated data files in a deterministic format.
- Record source metadata and import timestamp.

Acceptance criteria:

- Importers can regenerate the fixture data.
- Normalized outputs are deterministic.
- Import tests use checked-in fixtures, not live network calls.
- Runtime simulation does not require network access.

## 9. Validation Engine

Goal: validate project completeness and electrical feasibility independently
from simulation.

Tasks:

- Validate project references and topology consistency.
- Validate panel-to-mounting-group assignments.
- Validate string composition and MPPT connections.
- Compute string Voc and Vmpp bounds using panel datasheet temperature
  coefficients.
- Validate inverter voltage/current limits.
- Validate battery, BMS, cable, and blocking diode limits where data is
  available.
- Return machine-readable issue codes with severity and parameters.

Acceptance criteria:

- Validation can run without simulation.
- Invalid project references are reported without panics.
- Tests cover cold Voc, MPPT range, string current, mixed-orientation strings,
  incompatible battery/inverter voltage, cable voltage drop, and diode limits.

## 10. PV Production Simulation

Goal: compute hourly PV production before adding storage/grid behavior.

Tasks:

- Implement sun position calculations or adopt a well-tested Rust crate if one
  is suitable.
- Convert horizontal irradiance to plane-of-array irradiance per mounting group.
- Estimate cell temperature.
- Estimate panel DC power.
- Aggregate panel output into strings and inverter inputs.
- Apply configurable baseline losses.

Acceptance criteria:

- A fixed weather fixture produces deterministic hourly PV output.
- Daily, monthly, and yearly production aggregations are tested.
- Different tilt/azimuth values produce different outputs.

## 11. Inverter, Wiring, Diode, Battery, And Load Simulation

Goal: simulate a grid-tied hybrid residential system.

Tasks:

- Apply inverter MPPT limits, clipping, and conversion efficiency.
- Apply cable voltage drop and resistive losses.
- Apply blocking diode voltage drop and losses.
- Implement load profile consumption at hourly resolution.
- Implement battery charge/discharge behavior, SOC bounds, efficiency, and
  BMS power/current limits.
- Compute PV-to-load, PV-to-battery, PV-to-grid, battery-to-load, grid-to-load,
  and unmet load where applicable.

Acceptance criteria:

- Energy balance tests pass for simple systems.
- Battery SOC never violates configured min/max bounds.
- Grid import/export values are deterministic for fixed fixtures.
- Reports distinguish production, self-consumption, battery flow, and grid flow.

## 12. Reporting And Aggregation

Goal: expose simulation outputs in the views required by future UIs.

Tasks:

- Define hourly report records.
- Add aggregation by selected day, every day of year, month, and year.
- Include validation summary alongside simulation reports.
- Keep report labels language-neutral.

Acceptance criteria:

- Reports can answer: average production per day of year, hourly production for
  any day, monthly production, yearly production.
- Reports also include consumption, battery, grid import/export, and losses.
- Aggregation tests verify sums against hourly source records.

## 13. WASM Adapter

Goal: make the core usable from a future web app.

Tasks:

- Add `wasm-bindgen` or another suitable binding layer.
- Expose JSON-compatible functions for listing data, validating projects, and
  simulating projects.
- Keep large embedded data loading explicit to avoid surprising frontend costs.

Acceptance criteria:

- The WASM crate builds for `wasm32-unknown-unknown`.
- WASM validation and simulation results match direct `pv-core` results for
  fixture projects.

## 14. HTTP Service

Goal: provide a Docker-friendly API around the core.

Tasks:

- Implement a stateless HTTP service.
- Add endpoints for locations, catalogs, project normalization, validation, and
  simulation.
- Add request size limits and structured error responses.
- Add a Dockerfile and basic container configuration.

Acceptance criteria:

- The service runs locally.
- API tests confirm service results match direct `pv-core` results.
- The container starts and serves health/status plus API endpoints.
- The service does not persist projects server-side in v1.

## 15. Golden Fixtures And Regression Tests

Goal: make simulation changes reviewable and stable.

Tasks:

- Add small representative project fixtures:
  - minimal PV-only system
  - grid-tied PV with load
  - hybrid PV with battery
  - invalid electrical design
- Add golden expected outputs for key reports.
- Add migration fixtures for project schema versions.

Acceptance criteria:

- `cargo test --workspace` verifies fixtures.
- Intentional model changes require explicit golden output updates.
- Invalid fixture projects produce expected validation issue codes.

## 16. Future UI Preparation

Goal: avoid blocking the future visual editor and educational interface.

Tasks:

- Keep topology explicit in the project schema.
- Use stable component and connection IDs.
- Preserve enough metadata for a future canvas layout without requiring it now.
- Do not make the HTTP or WASM APIs depend on a specific frontend framework.

Acceptance criteria:

- A future UI can render components and connections from the project file.
- A future UI can add optional layout coordinates without breaking simulation.
- No current crate assumes TUI, web, desktop, or game UI ownership of the model.

