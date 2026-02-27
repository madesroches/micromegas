# Release Plan: Micromegas v0.21.0

## Overview

Release version 0.21.0 of Micromegas. This release features notebook cross-cell queries via WASM DataFusion, horizontal group cells, multi-query chart cells, a compact notebook UI redesign, WASM tracing support, Python CLI entry points, LZ4 Arrow IPC compression, and the new `micromegas-datafusion-extensions` crate. 259 commits since v0.20.0.

## Current Status

- **Version**: 0.21.0 (already bumped during v0.20.0 post-release)
- **Last Release**: v0.20.0 (February 2026)
- **Branch**: release
- **Commits since v0.20.0**: 259

## New Crate in release.py

`micromegas-datafusion-extensions` is a new published crate. It was added to `build/release.py` between Perfetto (Layer 7) and Analytics (Layer 8) in commit `25a335546`.

## Changes Since v0.20.0

### Major Features

1. **Notebook Cross-Cell Queries** (#815)
   - Cells with `dataSource: 'notebook'` execute SQL in-browser via WASM DataFusion against other cells' results
   - `execute_and_register` and `deregister_table` methods in WASM engine
   - Live download progress (rows/bytes) and execution time in cell title bars

2. **Horizontal Group Cell** (#821)
   - Side-by-side cell layout in notebooks
   - Variable value/onValueChange passed to children, combobox inline rendering
   - DataSourceField in child editors, running status and spinner
   - Drag-into-group, drag-out, and reorder support
   - 36 tests

3. **Multi-Query Chart Cells** (#749)
   - Per-query data sources in chart cells
   - `CellState.data` refactored from `Table | null` to `Table[]`
   - Multi-series Y-axis unit auto-scaling, tooltip XSS fix, portal tooltips

4. **Compact Notebook UI**
   - Borderless notebook UI with minimal visual chrome
   - Fade-on-idle behavior for cell metadata (three-state fade machine)
   - Restyled pagination bar, tables (minimal lines, prominent header), log cells
   - Always-visible loading spinner in cell status area

5. **WASM Tracing** (#817)
   - WASM support for `micromegas-tracing` and `telemetry-sink`
   - `Send + Sync` bounds on `EventSink` trait
   - `default-features = false` for `micromegas-tracing` in workspace
   - SHOW TABLES enabled in WASM query engine

6. **New Crate: `micromegas-datafusion-extensions`**
   - Extracts JSONB and histogram UDFs from `micromegas-analytics` into shared crate
   - WASM-compatible: enables browser-side SQL parity with server
   - Registers extension UDFs in WASM query engine
   - WASM integration tests for JSONB and histogram UDFs
   - Adds `jsonb_each` table function

### Other Enhancements

7. **Python CLI Entry Points**
   - `micromegas-query` and `micromegas-logout` as installed CLI entry points via `pip install micromegas`
   - Removed legacy single-purpose CLI scripts

8. **LZ4 Compression**
   - LZ4 compression for Arrow IPC streams (dramatically smaller network transfers)
   - Raised FlightSQL client max decoding message size to 100MB

9. **Notebook Enhancements**
   - Notebook pagination and per-cell auto-run (#823)
   - Transposed table cells and column override support (#868)
   - Process info notebook with analysis links and clamped time range
   - Add cell dialog: types sorted alphabetically
   - Fix unsaved edits detection via JSON snapshot on edit start
   - Copy and edit buttons in notebook view source panel
   - Row hiding in transposed table via right-click context menu
   - Switch cell selection from single-click to double-click
   - Stabilized hook references to prevent unnecessary re-renders
   - Notebook datasource option in dropdown variable cells (#861)
   - Lucide icons replacing single-letter cell type icons
   - Stop markdown link clicks from opening cell editor in notebook tables

10. **Security**
    - Bump rollup to 4.59.0 (CVE: arbitrary file write) across all JS packages
    - Upgrade ajv from 6.12.6 to 6.14.0 (CVE-2025-69873, dependabot #109)
    - Fix minimatch ReDoS via resolution override to v10.x (dependabot #104, #105)
    - Migrate Grafana eslint config to native flat config

11. **Build**
    - Gate server-only dependencies behind `server` feature flag on `micromegas-telemetry` and `micromegas` crates (#855)

12. **Bug Fixes**
    - Fix empty-string backward compatibility in `opt_uuid_from_string` for 0.20.0 clients (#850)
    - Fix health check URL in `start_analytics_web.py`
    - Fix chart cells defaulting to notebook instead of global data source
    - Fix multi-series chart Y-axis scale to visible series when hiding (#836)
    - Fix tooltip XSS, deduplicate SERIES_COLORS

### Documentation

- Add screenshots to notebook documentation pages
- Fix rustdoc warnings in analytics crate
- Add Interactive Notebooks for Observability presentation (Reveal.js + Micromegas brand theme)
- Add notebook docs: cell types reference, variables, execution model, web app overview
- Reorganize mkdocs nav into Analytics Web App, Integrations, and Operations sections
- Move SERIES_COLORS to shared chart-constants module

## Pre-Release Checklist

### 0. Fix release.py (DONE)

- [x] Add `micromegas-datafusion-extensions` to `build/release.py` between Perfetto and Analytics (commit `25a335546`)

### 1. Code Quality & Testing

#### Rust Workspace (from `rust/` directory)
- [ ] Run full CI pipeline: `python3 ../build/rust_ci.py`
- [ ] Ensure all tests pass: `cargo test`
- [ ] Code formatting check: `cargo fmt --check`
- [ ] Lint check: `cargo clippy --workspace -- -D warnings`
- [ ] Build all binaries: `cargo build --release`

#### Python Package (from `python/micromegas/` directory)
- [ ] Run Python tests: `poetry run pytest`
- [ ] Python code formatting: `poetry run black . --check`

#### Grafana Plugin (from `grafana/` directory)
- [ ] Install dependencies: `yarn install`
- [ ] Run linter: `yarn lint:fix`
- [ ] Run tests: `yarn test:ci`
- [ ] Build plugin: `yarn build`

#### Analytics Web App (from `analytics-web-app/` directory)
- [ ] Install dependencies: `yarn install`
- [ ] Run linter: `yarn lint`
- [ ] Run type check: `yarn type-check`
- [ ] Run tests: `yarn test`
- [ ] Build app: `yarn build`

### 2. Version Verification

All versions should already be at 0.21.0 from the v0.20.0 post-release bump:
- [ ] Verify workspace version in `rust/Cargo.toml` (currently 0.21.0 ✓)
- [ ] Verify Python version in `python/micromegas/pyproject.toml` (currently 0.21.0 ✓)
- [ ] Verify Grafana plugin version in `grafana/package.json` (currently 0.21.0 ✓)
- [ ] Verify analytics web app version in `analytics-web-app/package.json` (currently 0.21.0 ✓)

### 3. Documentation Updates

- [ ] Review git log: `git log --oneline v0.20.0..HEAD`
- [ ] Update `CHANGELOG.md` — move Unreleased entries to `## v0.21.0 - <date>` section
- [ ] Update `grafana/CHANGELOG.md` with version sync entry
- [ ] Update README roadmap for v0.21.0

### 4. Grafana Plugin Preparation

- [ ] Verify plugin.json metadata and version
- [ ] Build plugin archive: `./build-plugin.sh`

### 5. Git Preparation

- [ ] Create release tag: `git tag v0.21.0`
- [ ] Verify tag points to correct commit

## Release Process

### Phase 1: Rust Crates Release

```bash
cd /home/mad/micromegas/build
python3 release.py
```

Crates published in dependency order (60s grace period between publishes):
1. micromegas-datafusion-wasm
2. micromegas-derive-transit
3. micromegas-tracing-proc-macros
4. micromegas-transit
5. micromegas-tracing
6. micromegas-auth
7. micromegas-telemetry
8. micromegas-ingestion
9. micromegas-telemetry-sink
10. micromegas-perfetto
11. **micromegas-datafusion-extensions**
12. micromegas-analytics
13. micromegas-proc-macros
14. micromegas

Verification: all crates at v0.21.0 on crates.io.

### Phase 2: Python Library Release

From `python/micromegas/`:
- [ ] `poetry build`
- [ ] `poetry publish`
- [ ] Verify on PyPI

### Phase 3: Grafana Plugin Release

From `grafana/`:
- [ ] Build: `./build-plugin.sh`
- [ ] Tag: `git tag grafana-v0.21.0`
- [ ] Push tag to trigger GitHub Actions workflow
- [ ] Verify draft release with signed plugin archive

### Phase 4: Git Release

- [ ] Push tags: `git push origin v0.21.0 grafana-v0.21.0`
- [ ] Create GitHub release with tag v0.21.0
- [ ] Attach Grafana plugin archive
- [ ] Mark as latest release

### Phase 5: Post-Release Version Bump to 0.22.0

#### Rust (`rust/Cargo.toml`):
- [ ] Workspace version to 0.22.0
- [ ] All dependency versions to 0.22.0
- [ ] `rust/tracing/Cargo.toml`: proc-macros dependency to `^0.22`
- [ ] `rust/transit/Cargo.toml`: derive-transit dependency to `^0.22`

#### Other packages:
- [ ] `python/micromegas/pyproject.toml`: version to 0.22.0
- [ ] `grafana/package.json`: version to 0.22.0
- [ ] `analytics-web-app/package.json`: version to 0.22.0

#### Lock files:
- [ ] `cargo update` (from `rust/`)
- [ ] `yarn install` (from `grafana/`)
- [ ] `yarn install` (from `analytics-web-app/`)

- [ ] Commit version bump
- [ ] Push to release branch

### Phase 6: Merge to Main

- [ ] Create PR from release to main
- [ ] Merge after review

## GitHub Release Description

### Title

Micromegas v0.21.0 - Notebook Cross-Cell Queries & DataFusion Extensions

### Highlights

#### Notebook Cross-Cell Queries
Cells can now query results from other cells using the in-browser WASM DataFusion engine. Set `dataSource: 'notebook'` to execute SQL against any other cell's result set — no server round-trip required.

#### New Crate: micromegas-datafusion-extensions
Extracted JSONB and histogram UDFs into a shared, WASM-compatible crate. The WASM query engine now has full parity with the server-side analytics engine for SQL functions.

#### Horizontal Group Cells
New `hg` cell type for side-by-side cell layouts in notebooks. Supports variable passing, drag-and-drop reordering, and inline editing.

#### Multi-Query Chart Cells
Chart cells now support multiple independent queries, each with their own data source. `CellState.data` is now `Table[]` for full multi-result support.

#### Compact Notebook UI
Redesigned notebook with borderless, minimal chrome. Cell metadata fades when idle. Restyled tables, pagination, and log cells for a cleaner reading experience.

#### WASM Tracing
`micromegas-tracing` and `micromegas-telemetry-sink` now support WASM targets, enabling telemetry from browser-side query execution.

#### Python CLI
`micromegas-query` and `micromegas-logout` are now installed as CLI entry points via `pip install micromegas`.

## Rollback Plan

If issues are discovered after release:
- Yank problematic Rust crates: `cargo yank --vers 0.21.0 <crate-name>`
- Update GitHub release notes with issue documentation
- Prepare patch release v0.21.1 if critical issues found

## Open Questions

- None — release.py fix is straightforward and all versions are pre-bumped.
