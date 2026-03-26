# Release Plan: Micromegas v0.23.0

## Overview

Release version 0.23.0 of Micromegas. This release features JSONB array UDFs, CSV table provider, FlightSQL server builder, screens-as-code CLI, environment credential handling for object stores, DataFusion 52.4.0, notebook cell selection macros, and numerous security/dependency updates. 37 commits since v0.22.0.

## Current Status

- **Version**: 0.23.0 (already bumped during v0.22.0 post-release)
- **Last Release**: v0.22.0 (March 13, 2026)
- **Branch**: release
- **Commits since v0.22.0**: 37

## New Crates in release.py

None. No new published crates added since v0.22.0.

## Pre-Release Checklist

### 0. Verify release.py

- [x] No new crates to add — confirmed

### 1. Code Quality & Testing

#### Rust Workspace (from `rust/` directory)
- [ ] Run full CI pipeline: `python3 ../build/rust_ci.py`
- [ ] WASM: `cd rust/datafusion-wasm && python3 build.py --test`

#### Python Package (from `python/micromegas/` directory)
- [ ] `poetry run black . --check`
- [ ] `poetry run pytest` (server-dependent failures expected)

#### Grafana Plugin (from `grafana/` directory)
- [ ] `yarn install`
- [ ] `yarn lint:fix`
- [ ] `yarn test:ci`
- [ ] `yarn build`

#### Analytics Web App (from `analytics-web-app/` directory)
- [ ] `yarn install`
- [ ] `yarn lint`
- [ ] `yarn type-check`
- [ ] `yarn test`
- [ ] `yarn build`

### 2. Version Verification

- [x] `rust/Cargo.toml` workspace version = 0.23.0
- [x] `rust/datafusion-wasm/Cargo.toml` version = 0.23.0
- [x] `python/micromegas/pyproject.toml` version = 0.23.0
- [x] `grafana/package.json` version = 0.23.0
- [x] `analytics-web-app/package.json` version = 0.23.0

### 3. Documentation Updates

- [ ] Update `CHANGELOG.md` — move Unreleased to `## March 2026 - v0.23.0`
- [ ] Update `grafana/CHANGELOG.md` with version sync entry
- [ ] Update `README.md` roadmap if needed

### 4. Grafana Plugin Preparation

- [ ] Build plugin archive: `./build-plugin.sh` (from `grafana/`)

### 5. Git Preparation

- [ ] Commit changelog and doc updates
- [ ] Create release tag: `git tag v0.23.0`
- [ ] Create grafana tag: `git tag grafana-v0.23.0`
- [ ] Push: `git push origin release && git push origin v0.23.0 grafana-v0.23.0`

## Release Process

### Phase 1: Rust Crates

```bash
cd /home/mad/micromegas/build
python3 release.py
```

### Phase 2: Python Library

```bash
cd /home/mad/micromegas/python/micromegas
poetry build
poetry publish
```

### Phase 3: GitHub & Grafana Release

```bash
gh release create v0.23.0 \
  --title "Micromegas v0.23.0" \
  --notes "See CHANGELOG.md for details" \
  grafana/micromegas-micromegas-datasource.zip
```

### Phase 4: Post-Release Version Bump to 0.24.0

#### Rust
- `rust/Cargo.toml`: workspace version and all dependency versions to 0.24.0
- `rust/tracing/Cargo.toml`: proc-macros dep to `^0.24`
- `rust/transit/Cargo.toml`: derive-transit dep to `^0.24`
- `rust/datafusion-wasm/Cargo.toml`: version to 0.24.0, all micromegas deps to `^0.24`

#### Other packages
- `python/micromegas/pyproject.toml`: 0.24.0
- `grafana/package.json`: 0.24.0
- `analytics-web-app/package.json`: 0.24.0

#### Lock files
- `cargo update` (from `rust/`)
- `yarn install` (from `grafana/`)
- `yarn install` (from `analytics-web-app/`)
- `cd rust/datafusion-wasm && python3 build.py --test`

- [ ] Commit version bump
- [ ] Push to release branch

### Phase 5: Merge to Main

- [ ] Create PR from release to main
- [ ] Merge after CI passes
