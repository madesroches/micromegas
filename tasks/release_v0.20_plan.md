# Release Plan: Micromegas v0.20.0

## Overview
Release version 0.20.0 of Micromegas. This is a major release featuring client-side WASM query execution, configurable data sources, several new notebook cell types, client-side Perfetto trace generation, and a welcome landing page. 231 commits since v0.19.0 across 36 merged PRs.

## Current Status
- **Version**: 0.20.0 (already bumped during v0.19.0 post-release)
- **Last Release**: v0.19.0 (January 28, 2026)
- **Branch**: release (currently identical to main)
- **Commits since v0.19.0**: 231

## Changes Since v0.19.0

### Major Features

1. **Client-Side WASM Query Execution** (#806, #807, #808, #810)
   - `local_query` screen type running DataFusion SQL entirely in the browser via WebAssembly
   - Progressive row count and byte size display during source query fetch
   - Auto-run checkbox for query execution on text changes
   - Renamed `datafusion-wasm` to `micromegas-datafusion-wasm` with CI integration
   - Shared WASM builder Dockerfile stage for Docker builds
   - Workaround for DataFusion 52.1 `LimitPushdown` bug dropping LIMIT clauses

2. **Configurable Data Sources** (#793, #794, #800)
   - Server-side data source configuration with in-memory cache
   - Per-screen and per-cell data source selection
   - Data source selector on Processes, ProcessMetrics, and ProcessLog pages
   - Datasource variable type for notebook data source selection
   - Protected default data source from deletion
   - Admin-only data source management

3. **Client-Side Perfetto Trace Generation** (#784)
   - Replace server-side `generate_trace` endpoint with client-side trace fetching
   - gzip compression for all analytics-web-srv endpoints
   - Abort signal support for trace downloads
   - Shared `triggerTraceDownload` helper

4. **Welcome Landing Page** (#785)
   - Public-facing landing page for madesroches.github.io/micromegas
   - App screenshots, feature sections, integration highlights
   - GitHub Pages deployment

### Notebook Enhancements

5. **Perfetto Export Cell** (#771)
   - New notebook cell type for Perfetto trace export
   - Inline progress display, cache invalidation on timeRange change
   - Async tests and standardized error handling

6. **Expression Variable Type** (#782)
   - Adaptive `time_bin_duration` via expression variables
   - Allowlist-based AST evaluator (replaces `new Function()` for security)
   - `$duration_ms` and `$devicePixelRatio` bindings
   - 1ms/10ms snap levels

7. **Save & Diff Improvements** (#780)
   - Config diff modal with resizable display and cell reorder detection
   - Save buttons moved to title bar with brand colors
   - `hasUnsavedChanges` derived from config comparison instead of imperative setState
   - `useExposeSaveRef` hook extracted, duplicate `SaveFooter` removed from renderers
   - Escape key to close diff modal, a11y attributes

8. **Compact Variable Cells** (#779)
   - Variable cell input moved to title bar to reduce vertical space
   - Shared hook extraction, fix error visibility

9. **Column Hiding** (#790)
   - Right-click context menu to hide columns
   - `useColumnManagement` hook to deduplicate sort/hide logic

10. **Zoom Buttons** (#804)
    - Zoom in/out buttons added to time range control

### Other Enhancements

11. **Parallel Thread Span Execution** (#772)
    - Use `spawn_with_context` for parallel thread span execution in Perfetto traces

12. **Process Navigation** (#777)
    - Process Details link added to PivotButton navigation

13. **Admin Visibility** (#802)
    - Hide admin icon in sidebar for non-admin users

14. **Process List Deprecation** (#791)
    - Remove process list from available screen types, mark variant as deprecated

15. **Unreal Engine** (#786)
    - Support 32-bit and 64-bit metrics in telemetry plugin

### Performance Optimizations
(already in CHANGELOG)
- Parquet file content cache to reduce object storage reads (#757, #758)
- Parallelize JIT for Perfetto trace thread span generation (#759)
- Pipelined query planning for Perfetto trace generation (#759)

### Security Fixes

- Update bytes crate to 1.11.1 to fix CVE-2026-25541 (#767)
- Upgrade jsonwebtoken to 10.3 to fix type confusion vulnerability (#760)
- Fix dependabot security alerts: protobuf and time (#787)
- Bump cryptography from 46.0.3 to 46.0.5 (#801)

### Dependencies

- Update DataFusion to 52.1 and Arrow/Parquet to 57.2 (#756)
- Bump Arrow to 57.3 for WASM compatibility
- Bump datafusion-wasm edition to 2024

### Documentation & Tooling

- Add GoatCounter analytics to all public pages (#796)
- Link documentation site in crate READMEs and PyPI metadata (#798)
- Add branch-review skill for code review (#795)
- Fix GoatCounter missing from presentation builds (#797)

### Code Quality

- Remove old perf_report task folder
- Remove column name transformation in process list tables (#744)
- Delete orphaned queries.rs
- Refactor ProcessMetricsPage to single declarative execution effect
- Refactor analytics-web-srv main.rs into focused functions

## Pre-Release Checklist

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
All versions should already be at 0.20.0 from the v0.19.0 post-release bump:
- [ ] Verify workspace version in `rust/Cargo.toml`
- [ ] Verify Python version in `python/micromegas/pyproject.toml`
- [ ] Verify Grafana plugin version in `grafana/package.json`
- [ ] Verify analytics web app version in `analytics-web-app/package.json`
- [ ] Check that all workspace dependencies reference 0.20.0

### 4. Documentation Updates

#### CHANGELOG Updates
- [ ] Review git log: `git log --oneline v0.19.0..HEAD`
- [ ] Update `CHANGELOG.md` - add missing items to v0.20.0 section (see Changes section above for items not yet listed)
- [ ] Update `grafana/CHANGELOG.md` with version sync entry

#### README Updates
- [ ] Update README roadmap for v0.20.0

### 5. Grafana Plugin Preparation
- [ ] Verify plugin.json metadata and version
- [ ] Build plugin archive: `./build-plugin.sh`

### 6. Git Preparation
- [ ] Create release tag: `git tag v0.20.0`
- [ ] Verify tag points to correct commit

## Release Process

### Phase 1: Rust Crates Release
```bash
cd /home/mad/micromegas/build
python3 release.py
```

Crates published in dependency order (60s grace period between publishes):
1. micromegas-derive-transit
2. micromegas-tracing-proc-macros
3. micromegas-transit
4. micromegas-tracing
5. micromegas-auth
6. micromegas-telemetry
7. micromegas-ingestion
8. micromegas-telemetry-sink
9. micromegas-perfetto
10. micromegas-analytics
11. micromegas-proc-macros
12. micromegas

Verification: all crates at v0.20.0 on crates.io.

### Phase 2: Python Library Release
From `python/micromegas/`:
- [ ] `poetry build`
- [ ] `poetry publish`
- [ ] Verify on PyPI

### Phase 3: Grafana Plugin Release
From `grafana/`:
- [ ] Build: `./build-plugin.sh`
- [ ] Tag: `git tag grafana-v0.20.0`
- [ ] Push tag to trigger GitHub Actions workflow
- [ ] Verify draft release with signed plugin archive

### Phase 4: Git Release
- [ ] Push tags: `git push origin v0.20.0 grafana-v0.20.0`
- [ ] Create GitHub release with tag v0.20.0
- [ ] Attach Grafana plugin archive
- [ ] Mark as latest release

### Phase 5: Post-Release Version Bump to 0.21.0

#### Rust:
- [ ] `rust/Cargo.toml`: workspace version and all dependency versions to 0.21.0
- [ ] `rust/tracing/Cargo.toml`: proc-macros dependency to `^0.21`
- [ ] `rust/transit/Cargo.toml`: derive-transit dependency to `^0.21`

#### Other packages:
- [ ] `python/micromegas/pyproject.toml`: version to 0.21.0
- [ ] `grafana/package.json`: version to 0.21.0
- [ ] `analytics-web-app/package.json`: version to 0.21.0

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
Micromegas v0.20.0 - Client-Side WASM Queries & Configurable Data Sources

### Highlights

#### Client-Side WASM Query Execution
Run DataFusion SQL queries entirely in the browser via WebAssembly with the new `local_query` screen type. Features progressive result display, auto-run, and zero server load for exploratory queries.

#### Configurable Data Sources
Connect to multiple analytics backends with per-screen and per-cell data source selection. Includes admin management, a datasource variable type for notebooks, and protected default data source.

#### New Notebook Cell Types
- **Perfetto Export Cell**: Download Perfetto traces directly from notebooks with inline progress
- **Expression Variables**: Adaptive `time_bin_duration` using secure allowlist-based expression evaluation
- **Swimlane Cell**: Visualize concurrent events across categories
- **Property Timeline Cell**: Display property changes over time

#### Client-Side Perfetto Trace Generation
Replaced server-side trace generation with client-side fetching, enabling abort support and reducing server load. All endpoints now use gzip compression.

#### Save & Diff Experience
Config diff modal shows exactly what changed before saving, with cell reorder detection and brand-colored save buttons in the title bar.

#### Welcome Page
New public-facing landing page at madesroches.github.io/micromegas with app screenshots and feature highlights.

## Rollback Plan
If issues are discovered after release:
- Yank problematic Rust crates: `cargo yank --vers 0.20.0 <crate-name>`
- Update GitHub release notes with issue documentation
- Prepare patch release v0.20.1 if critical issues found

## Open Questions
- Any features that should be held back from this release?
