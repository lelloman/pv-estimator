# Development Guide

## Purpose

This document defines how the project should be developed as it grows from a
specification into a Rust core, data tooling, WASM adapter, HTTP service, and
future UIs.

The project is intentionally open-ended. Development should favor stable
abstractions, clear boundaries, and reviewable increments over quick local
shortcuts that make future directions harder.

## Source Of Truth

- `SPEC.md` defines product and technical requirements.
- `IMPLEMENTATION_PLAN.md` defines the ordered implementation roadmap.
- This file defines development workflow, contribution rules, branching, and CI
  expectations.

If a change affects product scope, architecture, public APIs, persistence, or
simulation semantics, update the relevant docs in the same pull request.

## Branching Strategy

Use short-lived feature branches from `dev`.

Branch roles:

- `master` is the protected release branch. Changes reach `master` only through
  pull requests from `dev`.
- `dev` is the protected integration branch for active development.
- Feature branches are created from `dev` and merged back into `dev` by pull
  request.

Branch naming:

- `docs/<topic>` for documentation-only changes
- `feat/<topic>` for new user-visible or public API capability
- `core/<topic>` for simulation/domain model work
- `data/<topic>` for catalog, weather, or importer work
- `fix/<topic>` for bug fixes
- `ci/<topic>` for CI and tooling
- `refactor/<topic>` for behavior-preserving internal changes

Rules:

- Keep branches focused on one coherent change.
- Rebase or merge from `dev` before review when the branch is stale.
- Open normal development pull requests into `dev`, not `master`.
- Open release/promotion pull requests from `dev` into `master` when `dev` is
  ready to ship.
- Do not mix unrelated refactors with feature work.
- Do not commit generated large datasets unless the implementation plan for that
  phase explicitly calls for them.

## Commit Style

Use concise conventional-style commit subjects:

- `docs: add development guide`
- `feat: add project schema model`
- `core: add typed electrical units`
- `data: add PVGIS fixture importer`
- `fix: reject missing load profile hours`
- `ci: add workspace test workflow`
- `refactor: split validation rule registry`

Each commit should leave the repo in a coherent state. Prefer several small
commits over one large mixed commit when the changes can be reviewed
independently.

## Pull Request Rules

Every pull request should state:

- what changed
- why the change is needed
- which part of `SPEC.md` or `IMPLEMENTATION_PLAN.md` it implements
- how it was tested
- any known limitations or follow-up work

Pull requests that change public APIs, project files, catalog formats, report
formats, or simulation behavior must include tests or a clear reason tests are
not possible yet.

## Abstraction Rules

The project should create explicit abstractions around likely axes of change:

- weather and irradiance sources
- equipment catalogs
- project file versions and migrations
- validation rules
- simulation stages
- report formats
- UI adapters
- service and WASM boundaries

Guidelines:

- Prefer named domain types over primitive strings, maps, or untyped JSON.
- Prefer explicit traits or enum-based extension points when a second
  implementation is plausible.
- Document why each major abstraction exists.
- Avoid abstractions that have no credible future variation.
- Keep the core independent from UI, HTTP, WASM, filesystem, and network
  concerns.

## Testing Policy

Expected test layers:

- unit tests for units, formulas, validation rules, and data normalization
- fixture tests for project JSON, weather data, and catalog data
- golden tests for representative simulations
- adapter tests proving HTTP/WASM outputs match `pv-core`
- schema migration tests when project file versions change

Minimum expectations before merging:

- Documentation-only changes must be reviewed for consistency.
- Rust changes must pass `cargo fmt --check`.
- Rust changes must pass `cargo test --workspace`.
- Linting should be added once the workspace exists; the expected baseline is
  `cargo clippy --workspace --all-targets -- -D warnings`.
- WASM changes must include a `wasm32-unknown-unknown` build check.
- HTTP service changes must include API tests or integration tests.

## CI Expectations

Once the Rust workspace exists, CI should run on every pull request.

Required CI policy:

- Every pull request into `dev` must pass CI before merge.
- Every pull request from `dev` into `master` must pass CI before merge.
- Direct pushes to `master` should be disabled.
- Direct pushes to `dev` should be avoided except for repository administration
  or emergency fixes.

Initial CI jobs:

- formatting check
- workspace tests
- clippy with warnings denied
- documentation link/check job where practical

Later CI jobs:

- WASM build
- HTTP service integration tests
- Docker image build
- importer fixture tests
- golden simulation regression tests
- schema migration tests

CI must not depend on live PVGIS, NASA POWER, or vendor datasheet downloads.
Networked data acquisition belongs in explicit importer commands, with checked-in
fixtures for tests.

## Data And Catalog Rules

Embedded commercial data must be traceable.

Catalog entries should include:

- manufacturer
- model
- source URL
- extraction date
- relevant datasheet values
- notes for assumptions or missing fields

Weather and solar data should include:

- source id
- source documentation URL
- import timestamp
- location id
- normalization assumptions
- quality flags where available

Do not silently invent missing datasheet values. If a required value is absent,
represent it as missing and let validation or simulation report the limitation.

## Compatibility Rules

Compatibility matters because saved projects should remain useful.

Rules:

- Project files must be versioned.
- Breaking schema changes require a migration path or an explicit documented
  decision.
- Public report fields should be renamed only with care.
- Stable issue codes should not be reused for different meanings.
- Catalog ids should remain stable once released.

## Review Priorities

Review should focus on:

- correctness of domain modeling
- unit safety
- electrical and energy balance assumptions
- extensibility of public interfaces
- clarity of validation/report errors
- test coverage for behavior and compatibility
- avoiding UI or network coupling in the core

Style issues matter, but correctness and long-term maintainability matter more.

