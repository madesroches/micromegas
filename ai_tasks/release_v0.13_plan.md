# Release Plan: Micromegas v0.13

## Overview
This document tracks the release of version 0.13 of Micromegas, including both Rust crates and the Python library.

## Current Status
- **Version**: Currently at 0.13.0-dev (post v0.12.0 release)
- **Last Release**: v0.12.0 (September 3, 2025)
- **Target**: v0.13.0
- **Branch**: Currently on `release` branch
- **Outstanding work**: Branch cleanup completed, ready for release preparation

## Pre-Release Checklist

### 1. Code Quality & Testing ✅ COMPLETED
- [x] Ensure main branch is up to date - **✅ Current**
- [x] Run full CI pipeline: `python3 build/rust_ci.py` (from `/rust` directory) - **✅ PASSED**
- [x] Ensure all tests pass: `cargo test` (from `/rust` directory) - **✅ 78 tests passed**
- [x] Run Python tests: `pytest` (from `/python/micromegas` directory) - **✅ 59 tests passed**
- [x] Code formatting check: `cargo fmt --check` (from `/rust` directory) - **✅ PASSED**
- [x] Python code formatting: `black .` (from `/python/micromegas` directory) - **✅ 7 files reformatted**
- [x] Lint check: `cargo clippy --workspace -- -D warnings` (from `/rust` directory) - **✅ PASSED**

### 2. Version Verification
Current versions should already be at 0.13.0:
- [ ] Verify workspace version in `/rust/Cargo.toml`
- [ ] Verify Python version in `/python/micromegas/pyproject.toml`
- [ ] Check that all workspace dependencies reference 0.13.0
- [ ] Verify web app version in `/analytics-web-app/package.json`

### 3. Documentation Updates
- [ ] **Update CHANGELOG.md** with v0.13 changes:
  - [ ] Add new section for v0.13.0 with release date
  - [ ] List all major features, bug fixes, and breaking changes since v0.12.0:
    - Dictionary encoding for properties columns (performance optimization)
    - Properties to JSONB UDF for efficient storage
    - Arrow string column accessor improvements
    - Schema evolution with incompatible partition retirement
    - Performance analysis and optimizations
  - [ ] Include any performance improvements or API changes
- [ ] **Update README files**:
  - [ ] Verify installation instructions show correct versions
  - [ ] Update any example code that references version numbers
  - [ ] Check that feature lists are current
- [ ] **Update documentation**:
  - [ ] Search for any hardcoded version references in docs
  - [ ] Update getting started guides if needed

### 4. Git Preparation
- [ ] Checkout main and create release branch: `git checkout main && git checkout -b release-v0.13.0`
- [ ] Tag the release: `git tag v0.13.0`

## Release Process

### Phase 1: Rust Crates Release
Use the automated release script with correct dependency order:

Execute: `python3 /build/release.py`

Expected order (11 crates):
1. **micromegas-derive-transit 0.13.0** - Transit derive macros
2. **micromegas-transit 0.13.0** - Data serialization framework
3. **micromegas-tracing-proc-macros 0.13.0** - Tracing procedural macros
4. **micromegas-tracing 0.13.0** - Core tracing library
5. **micromegas-telemetry 0.13.0** - Telemetry data structures
6. **micromegas-ingestion 0.13.0** - Data ingestion utilities
7. **micromegas-telemetry-sink 0.13.0** - Telemetry data sinks
8. **micromegas-perfetto 0.13.0** - Perfetto trace generation
9. **micromegas-analytics 0.13.0** - Analytics and query engine
10. **micromegas-proc-macros 0.13.0** - Top-level procedural macros
11. **micromegas 0.13.0** - Main public crate

### Phase 2: Python Library Release
From `/python/micromegas` directory:
- [ ] Build package: `poetry build`
- [ ] Publish to PyPI: `poetry publish`

### Phase 3: Git Release
- [ ] Push release branch: `git push origin release-v0.13.0`
- [ ] Push tags: `git push origin v0.13.0`
- [ ] **Create GitHub release**:
  - [ ] Use tag v0.13.0
  - [ ] Include comprehensive description with major features
  - [ ] List all published crates with links
  - [ ] Add installation instructions
  - [ ] Mark as latest release
- [ ] Create pull request for release branch

### Phase 4: Post-Release Version Bump to 0.14.0
Update all versions for next development cycle:

#### Rust Workspace Files:
- [ ] **`/rust/Cargo.toml`**:
  - [ ] Update `[workspace.package].version = "0.14.0"`
  - [ ] Update all workspace dependencies versions to `"0.14.0"`

#### Individual Crate Files:
- [ ] **`/rust/tracing/Cargo.toml`**: Update proc-macros dependency to `^0.14`
- [ ] **`/rust/transit/Cargo.toml`**: Update derive-transit dependency to `^0.14`

#### Python Package:
- [ ] **`/python/micromegas/pyproject.toml`**: Update to `version = "0.14.0"`

#### Web Application:
- [ ] **`/analytics-web-app/package.json`**: Update to `"version": "0.14.0"`

#### Lock Files:
- [ ] Regenerate Rust lock file: `cargo update`
- [ ] Regenerate Node.js lock file: `npm install`

#### Commit Version Bump:
- [ ] Commit version bump: `git commit -m "Bump version to 0.14.0 for next development cycle"`
- [ ] Push changes to release branch

## Rollback Plan
If issues are discovered after release:
- [ ] Yank problematic crates: `cargo yank --vers 0.13.0 <crate-name>`
- [ ] Remove problematic Python package version from PyPI (if possible)
- [ ] Document issues in GitHub release notes

## Key Features in v0.13.0
Based on recent commits since v0.12.0:

### Performance & Storage Optimizations
- **Dictionary encoding for properties columns**: Major performance optimization for repeated string values
- **Properties to JSONB UDF**: Efficient storage format for property data
- **Arrow string column accessor**: Improved with full dictionary encoding support

### Infrastructure & Schema Evolution
- **Incompatible partition retirement**: Admin feature for handling schema evolution
- **Performance analysis**: Comprehensive analysis of dictionary encoding effectiveness

### Bug Fixes & Improvements
- **Property accessor improvements**: Enhanced property_get function with dictionary-encoded arrays
- **Documentation updates**: Updated README and CHANGELOG with recent developments

## Dependencies Order (as per release.py)
The release script publishes crates in this specific order to respect dependencies:
1. micromegas-derive-transit (no internal deps)
2. micromegas-transit (depends on derive-transit)
3. micromegas-tracing-proc-macros (no internal deps)
4. micromegas-tracing (depends on proc-macros, transit)
5. micromegas-telemetry (depends on tracing, transit)
6. micromegas-ingestion (depends on telemetry, tracing, transit)
7. micromegas-telemetry-sink (depends on telemetry, tracing)
8. micromegas-analytics (depends on ingestion, telemetry, tracing, transit, perfetto)
9. micromegas-perfetto (depends on tracing, transit)
10. micromegas-proc-macros (depends on tracing, transit)
11. micromegas (public crate - depends on most others)

## Post-Release Tasks
- [ ] **Update CHANGELOG.md for next version**:
  - Add new `## [Unreleased]` section at the top
  - Move v0.13.0 section under released versions
- [ ] **Announce release**:
  - Social media/blog posts
  - Relevant Rust/observability communities
  - Update any package registry descriptions
- [ ] Monitor for any issues reported by users
- [ ] Prepare patch release if critical issues found

## Emergency Contacts
- Primary: Marc-Antoine Desroches <madesroches@gmail.com>
- Repository: https://github.com/madesroches/micromegas/

## Notes
- All crates use Apache-2.0 license
- All crates target Rust edition 2024
- Python library requires Python ^3.10
- Release script uses `cargo release` with automated publishing
- Current feature branch `properties_dict` needs to be merged before release