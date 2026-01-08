# Release Plan: Micromegas v0.18.0

## Overview
This document tracks the release of version 0.18.0 of Micromegas, which includes:
- **Reliability & Data Integrity** - Duplicate block cleanup, prevention of duplicate insertions
- **Ingestion Improvements** - HTTP error codes with client retry logic, Arrow IPC streaming
- **Analytics Enhancements** - SHOW TABLES support, LRU metadata cache, new UDFs
- **Web App Migration** - Next.js to Vite migration with improved navigation
- **Security Fixes** - CVE-2026-21441 (urllib3), qs/rsa vulnerabilities, esbuild fix

## Current Status
- **Version**: 0.18.0 (development)
- **Last Release**: v0.17.0 (December 2025)
- **Target**: v0.18.0
- **Branch**: release/v0.18.0
- **Commits since v0.17.0**: 30

## Changes Since v0.17.0

### Major Features

1. **Duplicate Block Cleanup** (#700, #691, #689)
   - Add periodic duplicate block cleanup to maintenance daemon
   - Prevent duplicate insertion for blocks, streams, and processes
   - Add delete_duplicate_blocks UDF for manual cleanup

2. **Ingestion Improvements** (#696, #699)
   - Add proper HTTP error codes and client retry logic
   - Fix queue_size going negative on timeout in http_event_sink

3. **Arrow IPC Streaming** (#685)
   - Implement Arrow IPC streaming for query API

4. **Analytics Web App Migration** (#667)
   - Migrate from Next.js to Vite for dynamic base path support

### Enhancements

- Enable SHOW TABLES and information_schema support (#687)
- Add global LRU metadata cache for partition metadata (#674)
- Add jsonb_object_keys UDF for JSON exploration (#673)
- Add property timeline feature for metrics visualization (#684)
- Metrics chart scaling and time units improvements (#681)
- Pivot split button for process view navigation (#682)
- Auto-refresh auth token on 401 API responses (#680)
- Fix custom queries being reset when filters change (#670)
- Improve process info navigation and cleanup trace screen (#669)

### Tracing & Instrumentation

- Fix async span parenting and add spawn_with_context helper (#675)
- Improve #[span_fn] rustdoc documentation (#676)
- Add thread block parsing trace and tooling config (#686)

### Python CLI

- HTTPS URI support and executable scripts (#683)

### Unreal Engine

- Add more metrics and process info to telemetry plugin (#672)

### Security Fixes

- Fix urllib3 decompression bomb vulnerability (CVE-2026-21441) (#695)
- Fix security vulnerabilities in qs and rsa dependencies (#693)
- Fix esbuild security vulnerability (GHSA-67mh-4wv8-2f99) (#671)

### Documentation & Planning

- Add unified observability for games presentation (#694)
- Design: Pivot split button UX for process views (#679)
- Design: Metrics zoom out feature for analytics web app (#678)
- Update changelog and readme for v0.18.0 work (#677)
- Add user-defined screens feature plan (#665)

## Pre-Release Checklist

### 1. Code Quality & Testing

#### Rust Workspace (from `/rust` directory)
- [x] Ensure main branch is up to date ✅
- [x] Run full CI pipeline: `python3 ../build/rust_ci.py` ✅ All passed
- [x] Ensure all tests pass: `cargo test` ✅ 270+ tests passed
- [x] Code formatting check: `cargo fmt --check` ✅ Passed
- [x] Lint check: `cargo clippy --workspace -- -D warnings` ✅ Passed
- [x] Build all binaries: `cargo build --release` ✅ Built in 5m01s

#### Python Package (from `/python/micromegas` directory)
- [x] Run Python tests: `poetry run pytest` ✅ 34 unit tests passed (50 integration tests require server)
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
- [x] Run tests: `yarn test` ✅ 51 tests passed
- [x] Build app: `yarn build` ✅ Built in 3.40s

### 2. Version Verification
Current versions should already be at 0.18.0:
- [x] Verify workspace version in `/rust/Cargo.toml` (should be 0.18.0) ✅ Confirmed: 0.18.0
- [x] Verify Python version in `/python/micromegas/pyproject.toml` ✅ Confirmed: 0.18.0
- [x] Verify Grafana plugin version in `/grafana/package.json` (should be 0.18.0) ✅ Confirmed: 0.18.0
- [x] Verify analytics web app version in `/analytics-web-app/package.json` ✅ Confirmed: 0.18.0
- [x] Check that all workspace dependencies reference 0.18.0 ✅ All 9 crates reference 0.18.0

### 3. Documentation Updates

#### CHANGELOG Updates
- [x] **Review git log**: `git log --oneline v0.17.0..HEAD` ✅ 30 commits reviewed
- [x] **Update main CHANGELOG.md** - Change [Unreleased] section to v0.18.0 with date ✅
- [x] **Update Grafana CHANGELOG**: `/grafana/CHANGELOG.md` ✅ Added v0.18.0 (version sync)

### 4. Grafana Plugin Preparation
- [x] **Verify plugin.json metadata**:
  - [x] Version matches package.json (0.18.0) ✅ Uses %VERSION% placeholder
  - [x] Author information is correct ✅ Marc-Antoine Desroches
  - [x] Links (documentation, issues) are correct ✅ Points to madesroches/micromegas
- [ ] **Build plugin archive**: Use `./build-plugin.sh` script (NOT manual tar)

### 5. Git Preparation
- [x] Create release tag: `git tag v0.18.0` ✅
- [x] Verify tag points to correct commit ✅ 8362fbc45

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
- [x] Verify all crates are published on crates.io at v0.18.0 ✅

### Phase 2: Python Library Release ✅
From `/python/micromegas` directory:
- [x] Build package: `poetry build` ✅
- [x] Publish to PyPI: `poetry publish` ✅
- [x] Verify package on PyPI: https://pypi.org/project/micromegas/ ✅ v0.18.0
- [x] Test installation: `pip install micromegas==0.18.0` ✅

### Phase 3: Grafana Plugin Release ✅
From `/grafana` directory:
- [x] Build and package: `./build-plugin.sh` ✅ 50MB archive
- [x] Verify archive contents are correct (files in micromegas-micromegas-datasource/, not dist/) ✅
- [x] Archive attached to GitHub release ✅

### Phase 4: Git Release ✅
- [x] Push tags: `git push origin v0.18.0` ✅
- [x] **Create GitHub release**: ✅
  - [x] Use tag v0.18.0 ✅
  - [x] Title: "Micromegas v0.18.0 - Reliability & Data Integrity" ✅
  - [x] Include comprehensive description with major features ✅
  - [x] Attach Grafana plugin archive ✅
  - [x] Mark as latest release ✅
- **Release URL**: https://github.com/madesroches/micromegas/releases/tag/v0.18.0

### Phase 5: Post-Release Version Bump to 0.19.0 ✅
Update all versions for next development cycle:

#### Rust Workspace Files:
- [x] **`/rust/Cargo.toml`**:
  - [x] Update `[workspace.package].version = "0.19.0"` ✅
  - [x] Update all workspace dependencies versions to `"0.19.0"` ✅

#### Individual Crate Files:
- [x] **`/rust/tracing/Cargo.toml`**: Update proc-macros dependency to `^0.19` ✅
- [x] **`/rust/transit/Cargo.toml`**: Update derive-transit dependency to `^0.19` ✅

#### Python Package:
- [x] **`/python/micromegas/pyproject.toml`**: Update to `version = "0.19.0"` ✅

#### Grafana Plugin:
- [x] **`/grafana/package.json`**: Update to `"version": "0.19.0"` ✅

#### Analytics Web App:
- [x] **`/analytics-web-app/package.json`**: Update to `"version": "0.19.0"` ✅

#### Lock Files:
- [x] Regenerate Rust lock file: `cargo update` (from `/rust` directory) ✅
- [x] Regenerate Grafana lock file: `yarn install` (from `/grafana` directory) ✅
- [x] Regenerate Analytics Web App lock file: `yarn install` (from `/analytics-web-app` directory) ✅

#### Commit Version Bump:
- [ ] Version bump committed
- [ ] Push to release branch

### Phase 6: Merge to Main
- [ ] Merge release/v0.18.0 branch to main
- [ ] Push main to origin

## Rollback Plan
If issues are discovered after release:
- [ ] Yank problematic Rust crates: `cargo yank --vers 0.18.0 <crate-name>`
- [ ] Remove problematic Python package version from PyPI (if possible)
- [ ] Update GitHub release notes with issue documentation
- [ ] Prepare patch release v0.18.1 if critical issues found

## Post-Release Tasks
- [ ] **Monitor for issues**:
  - [ ] Watch GitHub issues for bug reports
  - [ ] Check crates.io download stats
- [ ] **Prepare patch release** if critical issues found

## Release Summary for GitHub

### Title
Micromegas v0.18.0 - Reliability & Data Integrity

### Description Template
```markdown
## Highlights

### Duplicate Block Cleanup
Automatic maintenance daemon cleanup for duplicate blocks, plus prevention of duplicate insertions at the ingestion layer. Includes new `delete_duplicate_blocks` UDF for manual cleanup operations.

### Ingestion Reliability
Proper HTTP error codes with client retry logic for more resilient data ingestion. Fixed queue_size tracking issues in http_event_sink.

### Arrow IPC Streaming
New Arrow IPC streaming support for the query API, enabling efficient data transfer.

### Analytics Web App
Migrated from Next.js to Vite for better dynamic base path support and improved developer experience.

## All Changes

### Features
- Add periodic duplicate block cleanup to maintenance daemon (#700)
- Prevent duplicate insertion for blocks, streams, and processes (#691)
- Add delete_duplicate_blocks UDF (#689)
- Add proper HTTP error codes and client retry logic for ingestion (#696)
- Implement Arrow IPC streaming for query API (#685)
- Enable SHOW TABLES and information_schema support (#687)
- Add global LRU metadata cache for partition metadata (#674)
- Add jsonb_object_keys UDF (#673)
- Add property timeline feature for metrics visualization (#684)
- Migrate analytics web app from Next.js to Vite (#667)

### Enhancements
- Pivot split button for process view navigation (#682)
- Metrics chart scaling and time units improvements (#681)
- Auto-refresh auth token on 401 API responses (#680)
- Improve process info navigation and cleanup trace screen (#669)
- HTTPS URI support and executable scripts for Python CLI (#683)
- Add more metrics and process info to Unreal telemetry plugin (#672)

### Tracing
- Fix async span parenting and add spawn_with_context helper (#675)
- Improve #[span_fn] rustdoc documentation (#676)

### Bug Fixes
- Fix queue_size going negative on timeout in http_event_sink (#699)
- Fix custom queries being reset when filters change (#670)

### Security
- Fix urllib3 decompression bomb vulnerability (CVE-2026-21441) (#695)
- Fix security vulnerabilities in qs and rsa dependencies (#693)
- Fix esbuild security vulnerability (GHSA-67mh-4wv8-2f99) (#671)

## Installation

### Rust
```toml
[dependencies]
micromegas = "0.18.0"
```

### Python
```bash
pip install micromegas==0.18.0
```

### Grafana Plugin
Download and extract `micromegas-datasource-0.18.0.tar.gz` to your Grafana plugins directory.

### Docker
```bash
docker pull ghcr.io/madesroches/micromegas-telemetry-ingestion-srv:0.18.0
docker pull ghcr.io/madesroches/micromegas-flight-sql-srv:0.18.0
docker pull ghcr.io/madesroches/micromegas-analytics-web-srv:0.18.0
```
```

## Notes
- All crates use Apache-2.0 license
- Rust edition 2024, Python ^3.10, Node.js >=16
- Release script uses `cargo release` with 60s grace period between publishes
- 30 commits since v0.17.0
