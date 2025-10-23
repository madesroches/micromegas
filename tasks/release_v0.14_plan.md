# Release Plan: Micromegas v0.14.0

## Overview
This document tracks the release of version 0.14.0 of Micromegas, including both Rust crates and the Python library.

## Current Status
- **Version**: Currently at 0.14.0 (post v0.13.0 release)
- **Last Release**: v0.13.0 (September 18, 2025)
- **Target**: v0.14.0
- **Branch**: Currently on `release` branch
- **Outstanding work**: Need to prepare for release

## Pre-Release Checklist

### 1. Code Quality & Testing
- [ ] Ensure main branch is up to date
- [ ] Run full CI pipeline: `python3 build/rust_ci.py` (from `/rust` directory)
- [ ] Ensure all tests pass: `cargo test` (from `/rust` directory)
- [ ] Run Python tests: `pytest` (from `/python/micromegas` directory)
- [ ] Code formatting check: `cargo fmt --check` (from `/rust` directory)
- [ ] Python code formatting: `black .` (from `/python/micromegas` directory)
- [ ] Lint check: `cargo clippy --workspace -- -D warnings` (from `/rust` directory)

### 2. Version Verification
Current versions should already be at 0.14.0:
- [ ] Verify workspace version in `/rust/Cargo.toml`
- [ ] Verify Python version in `/python/micromegas/pyproject.toml`
- [ ] Check that all workspace dependencies reference 0.14.0
- [ ] Verify web app version in `/analytics-web-app/package.json`

### 3. Documentation Updates
- [ ] **Update CHANGELOG.md** with v0.14.0 changes:
  - [ ] Add new section for v0.14.0 with release date
  - [ ] Move all items from `[Unreleased]` section to v0.14.0 section
  - [ ] List all major features, bug fixes, and breaking changes since v0.13.0:
    - Complete properties to dictionary-encoded JSONB migration
    - Properties writing optimization with ProcessMetadata and BinaryColumnAccessor
    - Dictionary<Int32, Binary> support in jsonb_format_json UDF
    - SessionConfigurator for custom table registration
    - File existence validation in json_table_provider
    - Empty lakehouse partitions support
    - NULL value handling improvements in SQL-Arrow bridge
    - High-Frequency Observability presentation
    - Security updates (Vite vulnerability fixes)
    - Analytics Server Authentication Plan
  - [ ] Include any performance improvements or API changes
- [ ] **Update README files**:
  - [ ] Verify installation instructions show correct versions
  - [ ] Update any example code that references version numbers
  - [ ] Check that feature lists are current
- [ ] **Update documentation**:
  - [ ] Search for any hardcoded version references in docs
  - [ ] Update getting started guides if needed

### 4. Git Preparation
- [ ] Tag the release: `git tag v0.14.0`

## Release Process

### Phase 1: Rust Crates Release
Use the automated release script (from `/build` directory):
```bash
python3 release.py
```

**Crates to publish (in dependency order):**
1. [ ] **micromegas-derive-transit** - Transit derive macros
2. [ ] **micromegas-tracing-proc-macros** - Tracing procedural macros
3. [ ] **micromegas-transit** - Data serialization framework
4. [ ] **micromegas-tracing** - Core tracing library
5. [ ] **micromegas-telemetry** - Telemetry data structures
6. [ ] **micromegas-ingestion** - Data ingestion utilities
7. [ ] **micromegas-telemetry-sink** - Telemetry data sinks
8. [ ] **micromegas-perfetto** - Perfetto trace generation
9. [ ] **micromegas-analytics** - Analytics and query engine
10. [ ] **micromegas-proc-macros** - Top-level procedural macros
11. [ ] **micromegas** - Main public crate

**Verification:**
- [ ] Verify all crates are published on crates.io at v0.14.0

### Phase 2: Python Library Release
From `/python/micromegas` directory:
- [ ] Build package: `poetry build`
- [ ] Publish to PyPI: `poetry publish`
- [ ] Verify package on PyPI: https://pypi.org/project/micromegas/

### Phase 3: Git Release
- [ ] Push release branch: `git push origin release`
- [ ] Push tags: `git push origin v0.14.0`
- [ ] **Create GitHub release**:
  - [ ] Use tag v0.14.0
  - [ ] Include comprehensive description with major features
  - [ ] List all published crates with links
  - [ ] Add installation instructions
  - [ ] Mark as latest release

### Phase 4: Post-Release Version Bump to 0.15.0
Update all versions for next development cycle:

#### Rust Workspace Files:
- [ ] **`/rust/Cargo.toml`**:
  - [ ] Update `[workspace.package].version = "0.15.0"`
  - [ ] Update all workspace dependencies versions to `"0.15.0"`

#### Individual Crate Files:
- [ ] **`/rust/tracing/Cargo.toml`**: Update proc-macros dependency to `^0.15`
- [ ] **`/rust/transit/Cargo.toml`**: Update derive-transit dependency to `^0.15`

#### Python Package:
- [ ] **`/python/micromegas/pyproject.toml`**: Update to `version = "0.15.0"`

#### Web Application:
- [ ] **`/analytics-web-app/package.json`**: Update to `"version": "0.15.0"`

#### Lock Files:
- [ ] Regenerate Rust lock file: `cargo update` (from `/rust` directory)
- [ ] Regenerate Node.js lock file: `npm install` (from `/analytics-web-app` directory)

#### Commit Version Bump:
- [ ] Version bump committed: `git commit -m "Bump version to 0.15.0 for next development cycle"`
- [ ] Push to release branch if needed

## Rollback Plan
If issues are discovered after release:
- [ ] Yank problematic crates: `cargo yank --vers 0.14.0 <crate-name>`
- [ ] Remove problematic Python package version from PyPI (if possible)
- [ ] Document issues in GitHub release notes

## Key Features in v0.14.0
Based on commits and CHANGELOG since v0.13.0:

### Performance & Storage Optimizations
- **Complete properties to dictionary-encoded JSONB migration**: Finalized migration path for efficient storage (#521)
- **Properties writing optimization**: ProcessMetadata and BinaryColumnAccessor improvements (#522, #524)

### Analytics & Query Features
- **Dictionary<Int32, Binary> support**: Added to jsonb_format_json UDF (#536)
- **SessionConfigurator**: Custom table registration support (#531)
- **File existence validation**: Added to json_table_provider (#532)
- **property_get UDF enhancement**: Can now access JSONB columns (#520)
- **Empty lakehouse partitions support**: Proper handling of empty partitions (#537)

### Bug Fixes & Reliability
- **NULL value handling**: Fixed in SQL-Arrow bridge with integration tests (#541)
- **Null decoding errors**: Fixed in list_partitions table function (#540)
- **Null decoding errors**: Fixed for file_path in retire_partitions (#539)

### Documentation & Presentations
- **High-Frequency Observability presentation**: OSACON 2025 presentation added (#527, #528, #529, #533)
- **Presentation template update**: New Vite-based build (#525)

### Security & Dependencies
- **Vite security update**: Updated to 7.1.8 and 7.1.11 to fix vulnerabilities (#526, #542)
- **DataFusion and Arrow Flight**: Updated dependencies (#519)
- **General dependency updates**: cargo update (#530)

### Code Quality
- **Rustdoc fixes**: Fixed HTML tag warnings in analytics crate (#534)

### Future Work
- **Analytics Server Authentication Plan**: Planning for authentication features (#543)

## Dependencies Order (from release.py)
The release script publishes crates in this specific order to respect dependencies:
1. micromegas-derive-transit (no internal deps)
2. micromegas-tracing-proc-macros (no internal deps)
3. micromegas-transit (depends on derive-transit)
4. micromegas-tracing (depends on proc-macros, transit)
5. micromegas-telemetry (depends on tracing, transit)
6. micromegas-ingestion (depends on telemetry, tracing, transit)
7. micromegas-telemetry-sink (depends on telemetry, tracing)
8. micromegas-perfetto (depends on tracing, transit)
9. micromegas-analytics (depends on ingestion, telemetry, tracing, transit, perfetto)
10. micromegas-proc-macros (depends on tracing, analytics)
11. micromegas (public crate - depends on most others)

## Post-Release Tasks
- [ ] **Update CHANGELOG.md for next version**:
  - [ ] Add new `## [Unreleased]` section at the top
  - [ ] Move v0.14.0 section under released versions
- [ ] **Announce release**:
  - [ ] Social media/blog posts
  - [ ] Relevant Rust/observability communities
  - [ ] Update any package registry descriptions
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
- Grace period of 60 seconds between publishes to allow crates.io indexing

---

## Release Execution Log

### Pre-Release Phase
- [ ] Started: [Date/Time]
- [ ] Completed: [Date/Time]

### Release Phase
- [ ] Started: [Date/Time]
- [ ] Completed: [Date/Time]

### Post-Release Phase
- [ ] Started: [Date/Time]
- [ ] Completed: [Date/Time]

---

**Status**: Ready for execution
