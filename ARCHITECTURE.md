# Architecture And Extension Boundaries

## Purpose

This document captures the first set of abstractions for PV Estimator before the
Rust workspace is created. It exists to keep the core model stable while the
project explores multiple future directions: web UI, desktop UI, Docker service,
WASM use, richer simulation models, larger equipment catalogs, and additional
weather sources.

The goal is not abstraction for its own sake. Each boundary below exists because
the project has a plausible near-term reason to support more than one
implementation.

## Layering

The dependency direction should be:

1. `pv-core`: domain types, validation, simulation, reports, project schema.
2. `pv-data`: embedded catalogs, locations, and normalized weather data.
3. `pv-wasm`: WASM adapter around `pv-core` and `pv-data`.
4. `pv-service`: HTTP adapter around `pv-core` and `pv-data`.
5. `xtask`: development-time import, generation, and maintenance commands.

Rules:

- `pv-core` must not depend on `pv-data`, `pv-wasm`, `pv-service`, or `xtask`.
- `pv-core` must not perform network, filesystem, HTTP, UI, or process-global
  configuration work.
- Adapters may convert between transport formats and core types, but they must
  not own simulation or validation rules.
- Embedded data is an input to the core, not part of the core's behavior.

## Extension Axes

The first implementation should create explicit boundaries around these axes:

- weather and irradiance sources
- equipment catalogs
- project file versions and migrations
- validation rules
- simulation stages
- report formats and aggregations
- UI/API adapters
- data importers
- experimental surrogate models

These are expected to change earlier than the core identity of a PV system.

## Core Domain Contracts

The core should expose strongly typed domain contracts instead of primitive
maps.

Required contracts:

- `PvSystemProject`: versioned saved project model.
- `ProjectId`, `ComponentId`, `CatalogItemId`, `LocationId`, `WeatherSourceId`,
  and `EndpointId`: stable identifiers.
- `Location`: coordinates and metadata needed to select weather data.
- `WeatherDataset`: normalized hourly weather records plus source metadata.
- `EquipmentCatalog`: read-only lookup interface for catalog and custom
  equipment.
- `ValidationRule`: one independently testable validation check.
- `SimulationStage`: one ordered part of the simulation pipeline.
- `Report`: language-neutral simulation output with stable field names.

The exact Rust shapes may be traits, enums, structs, or modules, but each
contract should have a named owner and tests before it becomes widely used.

## Weather Boundary

Reason for abstraction: PVGIS and NASA POWER are both required initially, and
more sources may be added later.

Core-facing contract:

- receives normalized hourly weather data
- does not know the original provider wire format
- can report source id, source name, import metadata, and quality flags
- can handle missing optional fields through typed optional values
- follows `SOURCE_HARMONIZATION.md` for canonical fields, source mappings,
  missing-data policy, derivation policy, and source-disagreement handling

Importer-facing contract:

- provider-specific code converts raw source data into the normalized format
- importers live outside `pv-core`
- importer tests use fixtures, not live network calls

V1 implementations:

- PVGIS TMY importer
- NASA POWER importer
- embedded normalized datasets for supported locations

## Experimental Model Boundary

Reason for abstraction: the project may explore trained coordinate/time-based
surrogate models as a lossy replacement or supplement for large source-backed
weather datasets.

Rules:

- ML training code must stay outside `pv-core`.
- Production crates must not depend on training frameworks.
- A model is an experimental estimator until validation proves it is useful.
- Model outputs must identify whether they represent weather-field estimates,
  production estimates, or uncertainty bands.
- Source-backed datasets remain the evaluation reference during the research
  spike.

Current core-facing contract:

- `SourceModelRegistry` records model family, input/output shape, active source
  models, checkpoint URI, training data size, validation MAE, and coverage rule.
- `SourceAnnualPvEstimate` records one source model's annual PV estimate plus
  optional monthly estimates.
- `AnnualPvEnsembleEstimate` aggregates applicable source estimates into mean,
  low, high, half-spread, and spread-fraction bands for annual and monthly
  energy and irradiation values.
- `SourceModelCoverage` keeps global, PVGIS-gateway, and empirical mask coverage
  explicit.

Rules:

- Torch checkpoint execution remains outside production crates until packaging is
  decided.
- Runtime adapters may consume model outputs through the typed core contract, but
  training scripts remain in `experiments/ml-weather`.
- Annual and monthly estimates are the product-level target for this model
  family; hourly climate-normal values are intermediate data unless later
  validation proves a finer-grained use case.

## Equipment Catalog Boundary

Reason for abstraction: catalog data will come from embedded official datasheet
entries, project-local custom components, and likely future vendor/community
catalogs.

Core-facing contract:

- resolve equipment by stable id
- expose typed equipment records for panels, inverters, batteries, BMS devices,
  blocking diodes, and cables
- distinguish missing data from known zero values
- preserve source metadata for embedded commercial entries

V1 implementations:

- embedded EU residential catalog
- project-local custom equipment catalog
- merged read-only catalog view used by validation and simulation

## Project Persistence Boundary

Reason for abstraction: saved PV systems must survive schema evolution and be
usable across future clients.

Rules:

- project files are versioned JSON in v1
- schema version is required
- migrations are explicit functions, not ad hoc deserialization side effects
- internal domain types may differ from serialized DTOs when that protects
  compatibility
- unknown newer schema versions return structured errors

V1 contracts:

- deserialize project file DTO
- migrate supported older DTOs to the current DTO
- validate DTO references and convert to core domain model
- serialize current DTO

## Validation Boundary

Reason for abstraction: electrical validation will grow continuously as the
model gains more realism.

Validation rules should be independent units with:

- stable issue code
- severity
- affected component ids
- structured parameters
- deterministic output
- unit tests

Validation should not require a full production simulation. It may use shared
calculation helpers for string voltage, cable loss, and device limits.

V1 rule groups:

- project completeness
- topology references
- mounting group and string consistency
- string voltage/current limits
- inverter MPPT and DC/AC limits
- battery and BMS limits
- cable voltage drop and current assumptions
- blocking diode ratings
- weather and load profile completeness

## Simulation Boundary

Reason for abstraction: v1 uses hourly grid-tied hybrid simulation, but future
models may need finer time steps, off-grid operation, alternate irradiance
models, or more detailed battery behavior.

Simulation should be a pipeline of named stages. Each stage consumes an immutable
input context and writes explicit intermediate/output records.

V1 stages:

1. sun position and irradiance projection
2. panel temperature and DC panel output
3. string aggregation
4. inverter MPPT, clipping, and conversion
5. wiring and diode losses
6. load matching
7. battery/BMS charge and discharge
8. grid import/export
9. report aggregation

Rules:

- stages should be testable with fixtures
- energy flow outputs must support balance checks
- simulation settings must be explicit inputs
- adapters must not insert hidden defaults

## Reporting Boundary

Reason for abstraction: future clients will need charts, tables, exports, API
responses, and possibly educational explanations.

Reports should be language-neutral structured data.

Required report families:

- hourly records
- daily totals
- selected-day hourly view
- monthly totals
- yearly totals
- validation summary
- loss breakdown
- self-consumption and grid exchange summary

Rules:

- aggregation should be deterministic
- totals must be derivable from hourly records where practical
- report field names should remain stable once released

## Adapter Boundary

Reason for abstraction: the same core should serve WASM, HTTP, future desktop,
future web UI, and possible CLI/TUI tools.

Adapters may:

- parse requests
- serialize responses
- map transport errors to client-facing errors
- load embedded data
- expose endpoint/function names

Adapters must not:

- implement simulation behavior
- implement validation rules
- mutate project semantics
- depend on UI-specific assumptions inside `pv-core`

Initial adapters:

- WASM JSON-compatible API
- stateless HTTP API

## Error And Issue Boundary

Reason for abstraction: validation issues, import errors, schema errors, and API
errors need stable machine-readable handling.

Rules:

- use stable error/issue codes
- include structured parameters
- keep messages out of the core
- distinguish fatal errors from warnings and informational issues
- do not reuse an existing code for a different meaning

## Data Import Boundary

Reason for abstraction: importer code needs network and provider-specific
parsing, while runtime simulation should remain offline and deterministic.

Importer tooling may:

- call external APIs
- parse provider files
- generate normalized embedded data
- validate generated data
- record source metadata

Runtime code should:

- read embedded normalized data
- reject missing data clearly
- avoid live provider calls during simulation

## Abstraction Review Checklist

Before adding a major type, trait, or module, answer:

- What future variation does this support?
- Which crate owns it?
- Is it part of a public contract or private implementation detail?
- Can it be tested without HTTP, filesystem, network, or UI?
- Does it make the project easier to extend without hiding important behavior?
- Is a simple enum better than a trait for the current variation?
- Is a trait justified by multiple likely implementations?

Before merging phase 2, the project should have this document plus the
documentation baseline updated to reference it.
