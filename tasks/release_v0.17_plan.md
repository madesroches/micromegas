# Release Plan: Micromegas v0.17.0

## Overview
This document tracks the release of version 0.17.0 of Micromegas, which includes:
- **Analytics Web App Major Rework** - Complete redesign with dark theme, performance analysis, and Perfetto integration
- **Per-service Docker Images** - Containerized deployments with BASE_PATH support
- **Unreal Engine Enhancements** - API key authentication and scalability context
- **Security Fixes** - CVE-2025-66478 (Next.js), urllib3 vulnerabilities

## Current Status
- **Version**: 0.17.0 (development)
- **Last Release**: v0.16.0 (November 28, 2025)
- **Target**: v0.17.0
- **Branch**: release
- **Commits since v0.16.0**: 46

## Changes Since v0.16.0

### Major Features

1. **Analytics Web App Major Rework** (#621, #622, #623)
   - Complete UI redesign with dark theme and Micromegas branding
   - SQL query editor with syntax highlighting and query history
   - Interactive metrics charting

2. **Performance Analysis Screen** (#642, #643)
   - Thread coverage timeline visualization
   - Perfetto trace integration with split button (#660, #661)

3. **Time Range Picker** (#631)
   - Grafana-style relative and absolute time support
   - Time range passed through process navigation (#636)

4. **Process Metrics Screen** (#639)
   - Time-series charting for process metrics

5. **Per-service Docker Images** (#637, #649)
   - Modernized build scripts
   - Individual images for each service

6. **Deployment Configuration** (#650, #651, #654, #656, #658, #659)
   - BASE_PATH support for reverse proxy deployments
   - MICROMEGAS_PORT environment variable

### Enhancements

- Add process properties display panel (#634)
- Add multi-word search to process list and log screens (#632, #633)
- Allow custom limit values in process log view (#627, #628)
- Improve time column formatting in process logs (#624)
- Add schema documentation links to SQL panels (#635)
- Analytics web app UX improvements (#645, #647)

### Unreal Engine

- Add scalability and VSync context to telemetry (#625)
- Add API key authenticator (#618)
- Document FApiKeyAuthenticator (#619, #629)

### Security Fixes

- Fix CVE-2025-66478: Update Next.js to 15.5.7 (#626)
- Fix urllib3 security vulnerabilities (#641)
- Fix OIDC token validation bug (#641)

### Bug Fixes

- Fix UTF-8 user attribution headers with percent-encoding (#638)
- Handle empty MICROMEGAS_TELEMETRY_URL environment variable (#644)
- Fix base path routing for analytics web app (#658)
- Fix documentation dark mode readability (#648)

### Code Quality

- Fix rustdoc bare URL warnings in auth crate (#630)
- Update branding: replace Amber with Wheat as primary gold color (#623)
- Add branding assets and logos (#615, #616, #617)

## Pre-Release Checklist

### 1. Code Quality & Testing

#### Rust Workspace (from `/rust` directory)
- [x] Ensure main branch is up to date ✅
- [x] Run full CI pipeline: `python3 ../build/rust_ci.py` ✅ All passed
- [x] Ensure all tests pass: `cargo test` ✅ 245 tests passed
- [x] Code formatting check: `cargo fmt --check` ✅ Passed
- [x] Lint check: `cargo clippy --workspace -- -D warnings` ✅ Passed
- [ ] Build all binaries: `cargo build --release`

#### Python Package (from `/python/micromegas` directory)
- [x] Run Python tests: `poetry run pytest` ✅ 34 unit tests passed (47 integration tests require server)
- [x] Python code formatting: `poetry run black . --check` ✅ 41 files ok
- [ ] Verify Python dependencies are up to date

#### Grafana Plugin (from `/grafana` directory)
- [x] Install dependencies: `yarn install` ✅
- [x] Run linter: `yarn lint:fix` ✅ Passed
- [x] Run tests: `yarn test:ci` ✅ 47 tests passed
- [x] Build plugin: `yarn build` ✅ Build successful
- [x] Verify build artifacts in `dist/` directory ✅

#### Analytics Web App (from `/analytics-web-app` directory)
- [x] Install dependencies: `yarn install` ✅
- [x] Run linter: `yarn lint` ✅ Passed
- [x] Run type check: `yarn type-check` ✅ Passed
- [x] Run tests: `yarn test` ✅ 19 tests passed
- [x] Build app: `yarn build` ✅ Build successful

### 2. Version Verification
Current versions should already be at 0.17.0:
- [x] Verify workspace version in `/rust/Cargo.toml` (should be 0.17.0) ✅ Confirmed: 0.17.0
- [x] Verify Python version in `/python/micromegas/pyproject.toml` ✅ Confirmed: 0.17.0
- [x] Verify Grafana plugin version in `/grafana/package.json` (should be 0.17.0) ✅ Confirmed: 0.17.0
- [x] Verify analytics web app version in `/analytics-web-app/package.json` ✅ Fixed: 0.15.0 → 0.17.0
- [x] Check that all workspace dependencies reference 0.17.0 ✅ All 9 crates reference 0.17.0

### 3. Documentation Updates

#### CHANGELOG Updates
- [x] **Review git log**: `git log --oneline v0.16.0..HEAD` ✅ 49 commits reviewed
- [x] **Update main CHANGELOG.md** - Verify [Unreleased] section is empty and v0.17.0 is documented ✅
- [x] **Update Grafana CHANGELOG**: `/grafana/CHANGELOG.md` ✅ Added v0.17.0 section

### 4. Grafana Plugin Preparation
- [x] **Verify plugin.json metadata**:
  - [x] Version matches package.json (0.17.0) ✅ Uses %VERSION% placeholder
  - [x] Author information is correct ✅ Marc-Antoine Desroches
  - [x] Links (documentation, issues) are correct ✅ Points to madesroches/micromegas
- [x] **Build plugin archive**: Use `./build-plugin.sh` script (NOT manual tar) ✅ Built 51MB archive

### 5. Git Preparation
- [ ] Merge release branch to main (if applicable)
- [ ] Create release tag: `git tag v0.17.0`
- [ ] Verify tag points to correct commit

## Release Process

### Phase 1: Rust Crates Release
Use the automated release script (from `/build` directory):
```bash
cd /home/mad/micromegas/build
python3 release.py
```

**Crates to publish (in dependency order):**
1. [ ] **micromegas-derive-transit** - Transit derive macros
2. [ ] **micromegas-tracing-proc-macros** - Tracing procedural macros
3. [ ] **micromegas-transit** - Data serialization framework
4. [ ] **micromegas-tracing** - Core tracing library
5. [ ] **micromegas-auth** - Authentication providers
6. [ ] **micromegas-telemetry** - Telemetry data structures
7. [ ] **micromegas-ingestion** - Data ingestion utilities
8. [ ] **micromegas-telemetry-sink** - Telemetry data sinks
9. [ ] **micromegas-perfetto** - Perfetto trace generation
10. [ ] **micromegas-analytics** - Analytics and query engine
11. [ ] **micromegas-proc-macros** - Top-level procedural macros
12. [ ] **micromegas** - Main public crate

**Verification:**
- [ ] Verify all crates are published on crates.io at v0.17.0

### Phase 2: Python Library Release
From `/python/micromegas` directory:
- [ ] Build package: `poetry build`
- [ ] Publish to PyPI: `poetry publish`
- [ ] Verify package on PyPI: https://pypi.org/project/micromegas/
- [ ] Test installation: `pip install micromegas==0.17.0`

### Phase 3: Grafana Plugin Release
From `/grafana` directory:
- [ ] Build and package: `./build-plugin.sh`
- [ ] Move archive to release artifacts
- [ ] Verify archive contents are correct (files in micromegas-micromegas-datasource/, not dist/)

### Phase 4: Git Release
- [ ] Push tags: `git push origin v0.17.0`
- [ ] **Create GitHub release**:
  - [ ] Use tag v0.17.0
  - [ ] Title: "Micromegas v0.17.0 - Analytics Web App Rework"
  - [ ] Include comprehensive description with major features
  - [ ] Attach Grafana plugin archive
  - [ ] Mark as latest release

### Phase 5: Post-Release Version Bump to 0.18.0
Update all versions for next development cycle:

#### Rust Workspace Files:
- [ ] **`/rust/Cargo.toml`**:
  - [ ] Update `[workspace.package].version = "0.18.0"`
  - [ ] Update all workspace dependencies versions to `"0.18.0"`

#### Individual Crate Files:
- [ ] **`/rust/tracing/Cargo.toml`**: Update proc-macros dependency to `^0.18`
- [ ] **`/rust/transit/Cargo.toml`**: Update derive-transit dependency to `^0.18`

#### Python Package:
- [ ] **`/python/micromegas/pyproject.toml`**: Update to `version = "0.18.0"`

#### Grafana Plugin:
- [ ] **`/grafana/package.json`**: Update to `"version": "0.18.0"`

#### Analytics Web App:
- [ ] **`/analytics-web-app/package.json`**: Update to `"version": "0.18.0"`

#### Lock Files:
- [ ] Regenerate Rust lock file: `cargo update` (from `/rust` directory)
- [ ] Regenerate Grafana lock file: `yarn install` (from `/grafana` directory)
- [ ] Regenerate Analytics Web App lock file: `yarn install` (from `/analytics-web-app` directory)

#### Commit Version Bump:
- [ ] Version bump committed
- [ ] Push to main

## Rollback Plan
If issues are discovered after release:
- [ ] Yank problematic Rust crates: `cargo yank --vers 0.17.0 <crate-name>`
- [ ] Remove problematic Python package version from PyPI (if possible)
- [ ] Update GitHub release notes with issue documentation
- [ ] Prepare patch release v0.17.1 if critical issues found

## Post-Release Tasks
- [ ] **Monitor for issues**:
  - [ ] Watch GitHub issues for bug reports
  - [ ] Check crates.io download stats
- [ ] **Prepare patch release** if critical issues found

## Release Summary for GitHub

### Title
Micromegas v0.17.0 - Analytics Web App Rework

### Description Template
```markdown
## Highlights

### Analytics Web App Major Rework
Complete redesign of the analytics web application featuring:
- Dark theme with Micromegas branding
- SQL query editor with syntax highlighting and query history
- Grafana-style time range picker
- Interactive process metrics charting
- Performance analysis screen with thread coverage timeline

### Perfetto Trace Integration
Export and visualize performance traces with split button for browser viewing or download.

### Per-service Docker Images
Containerized deployments with individual images for each service and BASE_PATH support for reverse proxy configurations.

### Unreal Engine Enhancements
- API key authentication support
- Scalability and VSync context in telemetry

## All Changes

### Features
- Analytics web app major rework with dark theme (#621, #622, #623)
- Add performance analysis screen with thread coverage timeline (#642, #643)
- Add Perfetto trace integration with split button (#660, #661)
- Add Grafana-style time range picker (#631)
- Add process metrics screen with time-series charting (#639)
- Add per-service Docker images (#637, #649)
- Add BASE_PATH support for reverse proxy deployments (#650, #651, #654, #656, #658, #659)

### Enhancements
- Add process properties display panel (#634)
- Add multi-word search to process list and log screens (#632, #633)
- Allow custom limit values in process log view (#627, #628)
- Add schema documentation links to SQL panels (#635)
- Analytics web app UX improvements (#645, #647)

### Unreal Engine
- Add scalability and VSync context to telemetry (#625)
- Add API key authenticator (#618)
- Document API key authentication (#619, #629)

### Security
- Fix CVE-2025-66478: Update Next.js to 15.5.7 (#626)
- Fix urllib3 security vulnerabilities and OIDC token validation bug (#641)

### Bug Fixes
- Fix UTF-8 user attribution headers with percent-encoding (#638)
- Handle empty MICROMEGAS_TELEMETRY_URL environment variable (#644)
- Fix base path routing for analytics web app (#658)
- Fix documentation dark mode readability (#648)

### Code Quality
- Fix rustdoc bare URL warnings in auth crate (#630)

## Installation

### Rust
```toml
[dependencies]
micromegas = "0.17.0"
```

### Python
```bash
pip install micromegas==0.17.0
```

### Grafana Plugin
Download and extract `micromegas-datasource-0.17.0.tar.gz` to your Grafana plugins directory.

### Docker
```bash
docker pull ghcr.io/madesroches/micromegas-telemetry-ingestion-srv:0.17.0
docker pull ghcr.io/madesroches/micromegas-flight-sql-srv:0.17.0
docker pull ghcr.io/madesroches/micromegas-analytics-web-srv:0.17.0
```
```

## Notes
- All crates use Apache-2.0 license
- Rust edition 2024, Python ^3.10, Node.js >=16
- Release script uses `cargo release` with 60s grace period between publishes
- 46 commits since v0.16.0
