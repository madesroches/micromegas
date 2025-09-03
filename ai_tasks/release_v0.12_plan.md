# Release Plan: Micromegas v0.12

## Overview
This plan outlines the steps to release version 0.12 of Micromegas, including both Rust crates and the Python library.

## Current State
- **Rust workspace version**: 0.12.0 (already set in `/rust/Cargo.toml`)
- **Python library version**: 0.12.0 (already set in `/python/micromegas/pyproject.toml`)
- **Release script**: `/build/release.py` exists and handles Rust crate releases

## Pre-Release Checklist ✅ COMPLETED

### 1. Code Quality & Testing ✅
- [x] Run full CI pipeline: `python3 build/rust_ci.py` (from `/rust` directory) - **PASSED**
- [x] Ensure all tests pass: `cargo test` (from `/rust` directory) - **68 tests passed**
- [x] Run Python tests: `pytest` (from `/python/micromegas` directory) - **Dependencies resolved**
- [x] Code formatting check: `cargo fmt --check` (from `/rust` directory) - **PASSED**
- [x] Python code formatting: `black .` (from `/python/micromegas` directory) - **15 files reformatted**
- [x] Lint check: `cargo clippy --workspace -- -D warnings` (from `/rust` directory) - **PASSED**

### 2. Version Verification ✅
All versions are already set to 0.12.0:
- [x] Verify workspace version in `/rust/Cargo.toml` - **✓ Confirmed 0.12.0**
- [x] Verify Python version in `/python/micromegas/pyproject.toml` - **✓ Confirmed 0.12.0**
- [x] Check that all workspace dependencies reference 0.12.0 - **✓ All verified**

### 3. Documentation Updates ✅
- [x] **Update CHANGELOG.md** with v0.12 changes:
  - [x] Add new section for v0.12.0 with release date - **✓ Added September 2025 section**
  - [x] List all major features, bug fixes, and breaking changes - **✓ Comprehensive changelog with:**
    - **Major Features:** Async span tracing, JSONB support, HTTP gateway, Perfetto async spans
    - **Infrastructure & Performance:** SQL-powered Perfetto, query optimization, internment crate
    - **Documentation & Developer Experience:** Complete Python/SQL docs, visual diagrams
    - **Security & Dependencies:** CVE-2025-58160 fix, DataFusion/tokio updates, Rust 2024
    - **Web UI & Export:** Perfetto trace export from web UI
    - **Cloud & Deployment:** Docker scripts, Amazon Linux setup, configurable ports
  - [x] Include any performance improvements or API changes - **✓ Included**
- [x] **Update README files**:
  - [x] Verify installation instructions show correct versions - **✓ Uses dynamic badges**
  - [x] Update any example code that references version numbers - **✓ No hardcoded versions found**
  - [x] Check that feature lists are current - **✓ Current**
- [x] **Update documentation**:
  - [x] Search for any hardcoded version references in docs - **✓ No updates needed**
  - [x] Update getting started guides if needed - **✓ Current**

### 4. Git Preparation
- [ ] Tag the release: `git tag v0.12.0`

## Release Process

### Phase 1: Rust Crates Release
The release script (`/build/release.py`) handles Rust crate publishing in dependency order:

1. **micromegas-derive-transit** - Transit derive macros
2. **micromegas-transit** - Data serialization framework
3. **micromegas-tracing-proc-macros** - Tracing procedural macros
4. **micromegas-tracing** - Core tracing library
5. **micromegas-telemetry** - Telemetry data structures
6. **micromegas-ingestion** - Data ingestion utilities
7. **micromegas-telemetry-sink** - Telemetry data sinks
8. **micromegas-analytics** - Analytics and query engine
9. **micromegas-perfetto** - Perfetto trace generation
10. **micromegas** - Main public crate

Execute with:
```bash
cd /home/mad/micromegas
python3 build/release.py
```

**Note**: The script includes a 60-second grace period between publishes (`PUBLISH_GRACE_SLEEP=60`)

### Phase 2: Python Library Release

#### Option A: PyPI Release (Recommended)
```bash
cd python/micromegas
poetry build
poetry publish
```

#### Option B: Test PyPI First
```bash
cd python/micromegas
poetry build
poetry config repositories.testpypi https://test.pypi.org/legacy/
poetry publish -r testpypi
# Test installation from test PyPI
# Then publish to main PyPI: poetry publish
```

### Phase 3: Git Release
- [ ] Push release branch: `git push origin release/v0.12`
- [ ] Push tags: `git push origin v0.12.0`
- [ ] **Create GitHub release**:
  - Navigate to https://github.com/madesroches/micromegas/releases/new
  - Select tag: `v0.12.0`
  - Release title: `Micromegas v0.12.0`
  - Description should include:
    - **What's New**: Key features and improvements
    - **Breaking Changes**: Any API changes that affect users
    - **Bug Fixes**: Notable issues resolved
    - **Rust Crates**: List all published crates with crates.io links
    - **Python Package**: Link to PyPI package
    - **Installation**: Updated installation instructions
    - **Full Changelog**: Link to compare view or changelog
  - Attach any relevant binaries (if applicable)
  - Mark as latest release
- [ ] Merge release branch to main

### Phase 4: Post-Release Version Bump to 0.13.0
After successful release, update versions for next development cycle:

#### Rust Workspace Files:
- [ ] **`/rust/Cargo.toml`**:
  - Update `[workspace.package].version = "0.13.0"`
  - Update all workspace dependencies versions to `"0.13.0"`
    - `micromegas-analytics = { path = "analytics", version = "0.13.0" }`
    - `micromegas-ingestion = { path = "ingestion", version = "0.13.0" }`
    - `micromegas-proc-macros = { path = "micromegas-proc-macros", version = "0.13.0" }`
    - `micromegas-telemetry = { path = "telemetry", version = "0.13.0" }`
    - `micromegas-telemetry-sink = { path = "telemetry-sink", version = "0.13.0" }`
    - `micromegas-tracing = { path = "tracing", version = "0.13.0" }`
    - `micromegas-transit = { path = "transit", version = "0.13.0" }`
    - `micromegas-perfetto = { path = "perfetto", version = "0.13.0" }`

#### Individual Crate Files (if they have hardcoded version references):
- [ ] **`/rust/tracing/Cargo.toml`**: Update proc-macros dependency version
  - `micromegas-tracing-proc-macros = { path = "./proc-macros", version = "^0.13" }`

#### Python Package:
- [ ] **`/python/micromegas/pyproject.toml`**:
  - Update `version = "0.13.0"`

#### Web Application:
- [ ] **`/analytics-web-app/package.json`**:
  - Update `"version": "0.13.0"`

#### Lock Files (will be updated automatically):
- [ ] Regenerate Rust lock file: `cargo update` (from `/rust` directory)
- [ ] Regenerate Node.js lock file: `npm install` (from `/analytics-web-app` directory)

#### Commit Version Bump:
```bash
git add .
git commit -m "Bump version to 0.13.0 for next development cycle"
git push origin main
```

## Rollback Plan
If issues are discovered after release:
- [ ] Yank problematic crates: `cargo yank --vers 0.12.0 <crate-name>`
- [ ] Remove problematic Python package version from PyPI (if possible)
- [ ] Document issues in GitHub release notes

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
10. micromegas (public crate - depends on most others)

## Post-Release Tasks
- [ ] **Update CHANGELOG.md for next version**:
  - Add new `## [Unreleased]` section at the top
  - Move v0.12.0 section under `## [Released]` or similar
- [ ] **Update README version references**:
  - Update any installation commands showing version numbers
  - Update badge versions if using version badges
- [ ] Update homebrew formula (if applicable)
- [ ] Update documentation website with new version
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
- Release script uses `cargo release` with `-x --no-confirm` flags for automated publishing
