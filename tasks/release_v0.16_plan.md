# Release Plan: Micromegas v0.16.0

## Overview
This document tracks the release of version 0.16.0 of Micromegas, which includes:
- **DataFusion 51 upgrade** with LIMIT pushdown fixes
- **Multi-provider OIDC support** with Auth0 and token expiration fixes
- **HTTP Gateway** with authentication and security features
- **Analytics Web App** OIDC authentication
- **Grafana plugin improvements** (packaging fixes, Micromegas FlightSQL rename)

## Current Status
- **Version**: Currently at 0.16.0 (development)
- **Last Release**: v0.15.0 (November 14, 2025)
- **Target**: v0.16.0
- **Branch**: main
- **Commits since v0.15.0**: 19

## Changes Since v0.15.0

### Major Features
1. **HTTP Gateway** (#597) - New HTTP gateway with authentication and security features
2. **Analytics Web App OIDC** (#596) - OIDC authentication for analytics web app
3. **Multi-provider OIDC** (#608) - Support for multiple OIDC providers including Auth0
4. **DataFusion 51** (#598) - Major upgrade from 50.2.0 to 51.0.0

### Bug Fixes
- Fix ID token expiration and add multi-provider OIDC support (#608)
- Fix timestamp binding in retire_partition_by_metadata UDF (#606)
- Fix secureJsonData undefined error and rename plugin to Micromegas FlightSQL (#603)
- Handle empty incompatible partitions and fix thrift buffer sizing (#602)
- Fix Grafana plugin packaging and document release process (#601)
- Fix LIMIT pushdown in all TableProvider implementations (#600)
- Fix OIDC authentication and token refresh issues (#590)

### Performance & Optimization
- Optimize JSONB UDFs for dictionary-encoded column support (#593)

### Security
- Fix js-yaml prototype pollution vulnerability (CVE-2025-64718) (#592)

### Documentation & CI
- Document auth_provider parameter and deprecate headers in Python API (#595)
- Enable Claude to submit PR reviews and issue comments (#605)
- Claude PR Assistant workflow (#604)

## Pre-Release Checklist

### 1. Code Quality & Testing

#### Rust Workspace (from `/rust` directory)
- [x] Ensure main branch is up to date ✅
- [x] Run full CI pipeline: `python3 ../build/rust_ci.py` ✅ All tests passed
- [x] Ensure all tests pass: `cargo test` ✅ Passed
- [x] Code formatting check: `cargo fmt --check` ✅ Passed
- [x] Lint check: `cargo clippy --workspace -- -D warnings` ✅ Passed
- [x] Build all binaries: `cargo build --release` ✅ Passed

#### Python Package (from `/python/micromegas` directory)
- [x] Run Python tests: `poetry run pytest` ✅ 30 unit tests passed (integration tests require servers)
- [x] Python code formatting: `poetry run black .` ✅ Fixed test_admin.py
- [x] Verify Python dependencies are up to date ✅

#### Grafana Plugin (from `/grafana` directory)
- [x] Install dependencies: `yarn install` ✅
- [x] Run linter: `yarn lint:fix` ✅ Passed
- [x] Run tests: `yarn test:ci` ✅ 47 tests passed
- [x] Build plugin: `yarn build` ✅ Build successful
- [x] Verify build artifacts in `dist/` directory ✅

### 2. Version Verification
Current versions should already be at 0.16.0:
- [x] Verify workspace version in `/rust/Cargo.toml` (should be 0.16.0) ✅ Confirmed: 0.16.0
- [x] Verify Python version in `/python/micromegas/pyproject.toml` ✅ Set to 0.16.1 (0.16.0 was deleted from PyPI and cannot be reused)
- [x] Verify Grafana plugin version in `/grafana/package.json` (should be 0.16.0) ✅ Confirmed: 0.16.0
- [x] Check that all workspace dependencies reference 0.16.0 ✅ All 9 crates reference 0.16.0

### 3. Documentation Updates

#### CHANGELOG Updates
- [x] **Review git log**: `git log --oneline v0.15.0..HEAD` ✅ 19 commits reviewed
- [x] **Update main CHANGELOG.md** - Change [Unreleased] section to v0.16.0 with date ✅
- [x] **Update Grafana CHANGELOG**: `/grafana/CHANGELOG.md` ✅

### 4. Grafana Plugin Preparation
- [x] **Verify plugin.json metadata**:
  - [x] Version matches package.json (0.16.0) ✅ Uses %VERSION% placeholder, substituted at build time
  - [x] Author information is correct ✅ Marc-Antoine Desroches
  - [x] Links (documentation, issues) are correct ✅ Points to main micromegas repository
- [ ] **Build plugin archive**: Use `./build-plugin.sh` script (NOT manual tar)

### 5. Git Preparation
- [x] Create release tag: `git tag v0.16.0` ✅ Created
- [x] Verify tag points to correct commit ✅ Points to 5ebe30354 (Complete v0.16.0 pre-release checklist)

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
- [ ] Verify all crates are published on crates.io at v0.16.0

### Phase 2: Python Library Release
From `/python/micromegas` directory:
- [ ] Build package: `poetry build`
- [ ] Publish to PyPI: `poetry publish`
- [ ] Verify package on PyPI: https://pypi.org/project/micromegas/
- [ ] Test installation: `pip install micromegas==0.16.1`

**Note**: Python uses version 0.16.1 because 0.16.0 was previously deleted from PyPI and cannot be reused.

### Phase 3: Grafana Plugin Release
From `/grafana` directory:
- [ ] Build and package: `./build-plugin.sh`
- [ ] Move archive to release artifacts
- [ ] Verify archive contents are correct (files at root, not in dist/)

### Phase 4: Git Release
- [ ] Push tags: `git push origin v0.16.0`
- [ ] **Create GitHub release**:
  - [ ] Use tag v0.16.0
  - [ ] Title: "Micromegas v0.16.0 - HTTP Gateway & Multi-Provider OIDC"
  - [ ] Include comprehensive description with major features
  - [ ] Attach Grafana plugin archive
  - [ ] Mark as latest release

### Phase 5: Post-Release Version Bump to 0.17.0
Update all versions for next development cycle:

#### Rust Workspace Files:
- [ ] **`/rust/Cargo.toml`**:
  - [ ] Update `[workspace.package].version = "0.17.0"`
  - [ ] Update all workspace dependencies versions to `"0.17.0"`

#### Individual Crate Files:
- [ ] **`/rust/tracing/Cargo.toml`**: Update proc-macros dependency to `^0.17`
- [ ] **`/rust/transit/Cargo.toml`**: Update derive-transit dependency to `^0.17`

#### Python Package:
- [ ] **`/python/micromegas/pyproject.toml`**: Update to `version = "0.17.0"` (from 0.16.1)

#### Grafana Plugin:
- [ ] **`/grafana/package.json`**: Update to `"version": "0.17.0"`

#### Lock Files:
- [ ] Regenerate Rust lock file: `cargo update` (from `/rust` directory)
- [ ] Regenerate Grafana lock file: `yarn install` (from `/grafana` directory)

#### Commit Version Bump:
- [ ] Version bump committed
- [ ] Push to main

## Rollback Plan
If issues are discovered after release:
- [ ] Yank problematic Rust crates: `cargo yank --vers 0.16.0 <crate-name>`
- [ ] Remove problematic Python package version from PyPI (if possible)
- [ ] Update GitHub release notes with issue documentation
- [ ] Prepare patch release v0.16.1 if critical issues found

## Post-Release Tasks
- [ ] **Monitor for issues**:
  - [ ] Watch GitHub issues for bug reports
  - [ ] Check crates.io download stats
- [ ] **Prepare patch release** if critical issues found

## Release Summary for GitHub

### Title
Micromegas v0.16.0 - HTTP Gateway & Multi-Provider OIDC

### Description Template
```markdown
## Highlights

### HTTP Gateway
New HTTP gateway service with authentication and security features for web applications.

### Multi-Provider OIDC Support
Support for multiple OIDC identity providers simultaneously, including Auth0, Azure AD, and Google.

### DataFusion 51
Major upgrade to DataFusion 51.0.0 with improved query performance and LIMIT pushdown fixes.

### Analytics Web App
OIDC authentication support for the analytics web application.

## All Changes

### Features
- Add HTTP Gateway with Authentication and Security Features (#597)
- Add OIDC authentication to analytics web app (#596)
- Fix ID token expiration and add multi-provider OIDC support (#608)
- Upgrade DataFusion from version 50.2.0 to 51.0.0 (#598)

### Bug Fixes
- Fix timestamp binding in retire_partition_by_metadata UDF (#606)
- Fix secureJsonData undefined error and rename plugin to Micromegas FlightSQL (#603)
- Handle empty incompatible partitions and fix thrift buffer sizing (#602)
- Fix Grafana plugin packaging and document release process (#601)
- Fix LIMIT pushdown in all TableProvider implementations (#600)
- Fix OIDC authentication and token refresh issues (#590)

### Performance
- Optimize JSONB UDFs for dictionary-encoded column support (#593)

### Security
- Fix js-yaml prototype pollution vulnerability (CVE-2025-64718) (#592)

### Documentation
- Document auth_provider parameter and deprecate headers in Python API (#595)

## Installation

### Rust
```toml
[dependencies]
micromegas = "0.16.0"
```

### Python
```bash
pip install micromegas==0.16.1
```

### Grafana Plugin
Download and extract `micromegas-datasource-0.16.0.tar.gz` to your Grafana plugins directory.
```

## Notes
- All crates use Apache-2.0 license
- Rust edition 2024, Python ^3.10, Node.js >=16
- Release script uses `cargo release` with 60s grace period between publishes
- 19 commits since v0.15.0

## Release Execution Log
- [ ] Pre-Release: Not started
- [ ] Release: Not started
- [ ] Post-Release: Not started
