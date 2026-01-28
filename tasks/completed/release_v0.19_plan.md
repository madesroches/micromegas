# Release Plan: Micromegas v0.19.0

## Overview
This document tracks the release of version 0.19.0 of Micromegas, which includes:
- **User-Defined Screens** - Configurable screen types (table, notebook) with URL-driven state
- **Notebook Screen** - Multi-cell layout with SQL queries, charts, and syntax highlighting
- **Duplicate Cleanup Expansion** - Extended duplicate cleanup to streams and processes
- **Analytics Web App Improvements** - MVC refactor, histogram support, admin screens
- **Security Fixes** - lodash prototype pollution (CVE-2025-13465), diff, Grafana dependencies

## Current Status
- **Version**: 0.19.0 (development)
- **Last Release**: v0.18.0 (January 2026)
- **Target**: v0.19.0
- **Branch**: release
- **Commits since v0.18.0**: 31

## Changes Since v0.18.0

### Major Features

1. **User-Defined Screens** (#707, #726, #728, #729, #730, #736, #737)
   - Add user-defined screens feature with JSON configuration
   - Table screen type with generic SQL viewer
   - Notebook screen type with multi-cell layout
   - Notebook OCP refactoring and URL variable synchronization
   - Decouple URL param ownership from ScreenPage to renderers
   - Admin section with export/import screens

2. **Notebook Enhancements** (#731, #734, #735)
   - Add syntax highlighting to notebook cell editors
   - Delta-based URL handling for notebook variables and time range
   - Add copy/paste support for time ranges

3. **Duplicate Cleanup Expansion** (#721)
   - Add delete_duplicate_streams and delete_duplicate_processes UDFs

4. **Histogram & Chart Improvements** (#720, #718, #732)
   - Add expand_histogram table function and bar chart toggle
   - MVC view state refactor and XYChart generalization
   - Unify chart and property timeline queries

### Enhancements

- Enable dictionary encoding preservation for web app (#727)
- Consolidate API endpoints under /api prefix (#711)
- Add dynamic page titles to analytics web app (#712)
- Disable source maps in production builds (#710)
- Fix blank page on hard refresh for deep URLs (#713)
- Migrate remaining pages to useScreenConfig and remove useTimeRange (#719)
- Add micromegas_app database creation to service startup (#705)

### Security Fixes

- Fix lodash prototype pollution vulnerability (CVE-2025-13465) (#725)
- Fix Dependabot alert #91: upgrade diff to 8.0.3 (#708)
- Fix dependabot alerts for Grafana plugin dependencies (#704)
- Fix 4 dependabot security alerts (#703)

### Documentation & Planning

- Add plans for unified metrics query and dictionary preservation (#724)
- Add notebook screen design and generalized metrics chart plan (#716)
- Update changelog and readme with unreleased changes (#722)
- Update unified observability presentation slides (#706)
- Add unified observability presentation link and fix changelog categorization (#702)

## Pre-Release Checklist

### 1. Code Quality & Testing

#### Rust Workspace (from `/rust` directory)
- [x] Ensure main branch is up to date ✅
- [x] Run full CI pipeline: `python3 ../build/rust_ci.py` ✅ All passed (fmt, clippy, machete, tests)
- [x] Ensure all tests pass: `cargo test` ✅ 313 tests passed
- [x] Code formatting check: `cargo fmt --check` ✅ Passed
- [x] Lint check: `cargo clippy --workspace -- -D warnings` ✅ Passed
- [x] Build all binaries: `cargo build --release` ✅ Built in 4m34s

#### Python Package (from `/python/micromegas` directory)
- [x] Run Python tests: `poetry run pytest` ✅ 34 unit tests passed (51 integration tests require server)
- [x] Python code formatting: `poetry run black . --check` ✅ 42 files unchanged
- [x] Verify Python dependencies are up to date ✅

#### Grafana Plugin (from `/grafana` directory)
- [x] Install dependencies: `yarn install` ✅
- [x] Run linter: `yarn lint:fix` ✅ Passed
- [x] Run tests: `yarn test:ci` ✅ 47 tests passed
- [x] Build plugin: `yarn build` ✅ Build successful
- [x] Verify build artifacts in `dist/` directory ✅ All platform binaries present

#### Analytics Web App (from `/analytics-web-app` directory)
- [x] Install dependencies: `yarn install` ✅
- [x] Run linter: `yarn lint` ✅ Passed
- [x] Run type check: `yarn type-check` ✅ Passed
- [x] Run tests: `yarn test` ✅ 355 tests passed
- [x] Build app: `yarn build` ✅ Built in 3.81s

### 2. Version Verification
Current versions should already be at 0.19.0:
- [x] Verify workspace version in `/rust/Cargo.toml` (should be 0.19.0) ✅ Confirmed: 0.19.0
- [x] Verify Python version in `/python/micromegas/pyproject.toml` ✅ Confirmed: 0.19.0
- [x] Verify Grafana plugin version in `/grafana/package.json` (should be 0.19.0) ✅ Confirmed: 0.19.0
- [x] Verify analytics web app version in `/analytics-web-app/package.json` ✅ Confirmed: 0.19.0
- [x] Check that all workspace dependencies reference 0.19.0 ✅ All 10 references confirmed

### 3. Documentation Updates

#### CHANGELOG Updates
- [x] **Review git log**: `git log --oneline v0.18.0..HEAD` ✅ 31 commits reviewed
- [x] **Update main CHANGELOG.md** - Change [Unreleased] section to v0.19.0 with date ✅
- [x] **Update Grafana CHANGELOG**: `/grafana/CHANGELOG.md` ✅ Version sync entry added

### 4. Grafana Plugin Preparation
- [x] **Verify plugin.json metadata**:
  - [x] Version matches package.json (0.19.0) ✅ Uses %VERSION% placeholder
  - [x] Author information is correct ✅ Marc-Antoine Desroches
  - [x] Links (documentation, issues) are correct ✅ Points to madesroches/micromegas
- [x] **Build plugin archive**: Use `./build-plugin.sh` script (NOT manual tar) ✅ 51MB archive

### 5. Git Preparation
- [x] Create release tag: `git tag v0.19.0` ✅
- [x] Verify tag points to correct commit ✅ b70b25d8d

## Release Process

### Phase 1: Rust Crates Release
Use the automated release script (from `/build` directory):
```bash
cd /home/mad/micromegas/build
python3 release.py
```

**Crates to publish (in dependency order):**
1. [x] **micromegas-derive-transit** - Transit derive macros ✅
2. [x] **micromegas-tracing-proc-macros** - Tracing procedural macros ✅
3. [x] **micromegas-transit** - Data serialization framework ✅
4. [x] **micromegas-tracing** - Core tracing library ✅
5. [x] **micromegas-auth** - Authentication providers ✅
6. [x] **micromegas-telemetry** - Telemetry data structures ✅
7. [x] **micromegas-ingestion** - Data ingestion utilities ✅
8. [x] **micromegas-telemetry-sink** - Telemetry data sinks ✅
9. [x] **micromegas-perfetto** - Perfetto trace generation ✅
10. [x] **micromegas-analytics** - Analytics and query engine ✅
11. [x] **micromegas-proc-macros** - Top-level procedural macros ✅
12. [x] **micromegas** - Main public crate ✅

**Verification:**
- [x] Verify all crates are published on crates.io at v0.19.0 ✅

### Phase 2: Python Library Release ✅
From `/python/micromegas` directory:
- [x] Build package: `poetry build` ✅
- [x] Publish to PyPI: `poetry publish` ✅
- [x] Verify package on PyPI: https://pypi.org/project/micromegas/ ✅
- [x] Test installation: `pip install micromegas==0.19.0` ✅

### Phase 3: Grafana Plugin Release ✅
From `/grafana` directory:
- [x] Build and package: `./build-plugin.sh` ✅ 51MB archive
- [x] Verify archive contents are correct (files in micromegas-micromegas-datasource/, not dist/) ✅
- [x] Archive attached to GitHub release ✅

### Phase 4: Git Release ✅
- [x] Push tags: `git push origin v0.19.0` ✅
- [x] **Create GitHub release**: ✅
  - [x] Use tag v0.19.0 ✅
  - [x] Title: "Micromegas v0.19.0 - User-Defined Screens & Notebooks" ✅
  - [x] Include comprehensive description with major features ✅
  - [x] Attach Grafana plugin archive ✅
  - [x] Mark as latest release ✅
- **Release URL**: https://github.com/madesroches/micromegas/releases/tag/v0.19.0

### Phase 5: Post-Release Version Bump to 0.20.0 ✅
Update all versions for next development cycle:

#### Rust Workspace Files:
- [x] **`/rust/Cargo.toml`**:
  - [x] Update `[workspace.package].version = "0.20.0"` ✅
  - [x] Update all workspace dependencies versions to `"0.20.0"` ✅

#### Individual Crate Files:
- [x] **`/rust/tracing/Cargo.toml`**: Update proc-macros dependency to `^0.20` ✅
- [x] **`/rust/transit/Cargo.toml`**: Update derive-transit dependency to `^0.20` ✅

#### Python Package:
- [x] **`/python/micromegas/pyproject.toml`**: Update to `version = "0.20.0"` ✅

#### Grafana Plugin:
- [x] **`/grafana/package.json`**: Update to `"version": "0.20.0"` ✅

#### Analytics Web App:
- [x] **`/analytics-web-app/package.json`**: Update to `"version": "0.20.0"` ✅

#### Lock Files:
- [x] Regenerate Rust lock file: `cargo update` (from `/rust` directory) ✅
- [x] Regenerate Grafana lock file: `yarn install` (from `/grafana` directory) ✅
- [x] Regenerate Analytics Web App lock file: `yarn install` (from `/analytics-web-app` directory) ✅

#### Commit Version Bump:
- [x] Version bump committed ✅ (9a6f8c4b9)
- [x] Push to release branch ✅

#### README Update:
- [x] Update README roadmap for v0.19.0 release ✅

### Phase 6: Merge to Main
- [ ] Create PR to merge release to main
- [ ] Merge PR after review
- [ ] Push main to origin

## Rollback Plan
If issues are discovered after release:
- [ ] Yank problematic Rust crates: `cargo yank --vers 0.19.0 <crate-name>`
- [ ] Remove problematic Python package version from PyPI (if possible)
- [ ] Update GitHub release notes with issue documentation
- [ ] Prepare patch release v0.19.1 if critical issues found

## Post-Release Tasks
- [ ] **Monitor for issues**:
  - [ ] Watch GitHub issues for bug reports
  - [ ] Check crates.io download stats
- [ ] **Prepare patch release** if critical issues found

## Release Summary for GitHub

### Title
Micromegas v0.19.0 - User-Defined Screens & Notebooks

### Description Template
```markdown
## Highlights

### User-Defined Screens
Configurable screen types driven by JSON configuration, enabling custom analytics views without code changes. Includes table screens with generic SQL viewers and an admin section for exporting/importing screen definitions.

### Notebook Screen
Multi-cell notebook layout supporting SQL queries, charts, and markdown. Features syntax highlighting, delta-based URL state management, and copy/paste support for time ranges.

### Duplicate Cleanup Expansion
Extended duplicate cleanup UDFs to cover streams and processes in addition to blocks, completing the data integrity tooling.

### Analytics Web App
MVC view state refactor with XYChart generalization, histogram expansion with bar chart toggle, dictionary encoding preservation, consolidated /api endpoints, and dynamic page titles.

## All Changes

### Features
- Add user-defined screens feature (#707)
- Add table screen type with generic SQL viewer (#726)
- Add notebook screen type with multi-cell layout (#728)
- Refactor notebook cells to follow Open-Closed Principle (#729)
- Notebook OCP refactoring and URL variable synchronization (#730)
- Add syntax highlighting to notebook cell editors (#731)
- Delta-based URL handling for notebook variables and time range (#734)
- Add copy/paste support for time ranges (#735)
- Decouple URL param ownership from ScreenPage to renderers (#736)
- Add admin section with export/import screens (#737)
- Add expand_histogram table function and bar chart toggle (#720)
- Add delete_duplicate_streams and delete_duplicate_processes (#721)
- Unify chart and property timeline queries (#732)

### Enhancements
- Enable dictionary encoding preservation for web app (#727)
- MVC view state refactor and XYChart generalization (#718)
- Migrate remaining pages to useScreenConfig and remove useTimeRange (#719)
- Consolidate API endpoints under /api prefix (#711)
- Add dynamic page titles to analytics web app (#712)
- Disable source maps in production builds (#710)
- Add micromegas_app database creation to service startup (#705)

### Bug Fixes
- Fix blank page on hard refresh for deep URLs (#713)

### Security
- Fix lodash prototype pollution vulnerability (CVE-2025-13465) (#725)
- Fix Dependabot alert #91: upgrade diff to 8.0.3 (#708)
- Fix dependabot alerts for Grafana plugin dependencies (#704)
- Fix 4 dependabot security alerts (#703)

## Installation

### Rust
```toml
[dependencies]
micromegas = "0.19.0"
```

### Python
```bash
pip install micromegas==0.19.0
```

### Grafana Plugin
Download and extract `micromegas-datasource-0.19.0.tar.gz` to your Grafana plugins directory.

### Docker
```bash
docker pull ghcr.io/madesroches/micromegas-telemetry-ingestion-srv:0.19.0
docker pull ghcr.io/madesroches/micromegas-flight-sql-srv:0.19.0
docker pull ghcr.io/madesroches/micromegas-analytics-web-srv:0.19.0
```
```

## Notes
- All crates use Apache-2.0 license
- Rust edition 2024, Python ^3.10, Node.js >=16
- Release script uses `cargo release` with 60s grace period between publishes
- 31 commits since v0.18.0
