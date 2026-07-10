# Release Plan: Micromegas v0.24.0

## Overview

Release version 0.24.0 of Micromegas. This release features the `parse_block` table UDF, screens-as-code diff output, flamechart key-release fix, DataFusion 52.5, `rand` 0.9 migration, pyarrow ^23, and numerous Dependabot security updates. 19 commits since v0.23.0.

## Current Status

- **Version**: 0.24.0 (already bumped during v0.23.0 post-release)
- **Last Release**: v0.23.0 (March 26, 2026)
- **Branch**: release
- **Commits since v0.23.0**: 19

## New Crates in release.py

None. No new published crates added since v0.23.0.

## Pre-Release Checklist

### 0. Verify release.py

- [ ] No new crates to add — confirm

### 1. Code Quality & Testing

#### Rust Workspace (from `rust/` directory)
- [ ] Run full CI pipeline: `python3 ../build/rust_ci.py`
- [ ] WASM: `cd rust/datafusion-wasm && python3 build.py --test`

#### Python Package (from `python/micromegas/` directory)
- [ ] `poetry run black . --check`
- [ ] `poetry run pytest`

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

- [ ] `rust/Cargo.toml` workspace version = 0.24.0
- [ ] `rust/datafusion-wasm/Cargo.toml` version = 0.24.0
- [ ] `python/micromegas/pyproject.toml` version = 0.24.0
- [ ] `grafana/package.json` version = 0.24.0
- [ ] `analytics-web-app/package.json` version = 0.24.0

### 3. Documentation Updates

- [ ] Update `CHANGELOG.md` — move Unreleased to `## April 2026 - v0.24.0`
- [ ] Update `grafana/CHANGELOG.md` with version sync entry
- [ ] Update `README.md` roadmap with v0.24.0 highlights

### 4. Grafana Plugin Preparation

- [ ] Build plugin archive: `./build-plugin.sh` (from `grafana/`)

### 5. Git Preparation

- [ ] Commit changelog and doc updates
- [ ] Create release tag: `git tag v0.24.0`
- [ ] Create grafana tag: `git tag grafana-v0.24.0`
- [ ] Push: `git push origin release && git push origin v0.24.0 grafana-v0.24.0`

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
gh release create v0.24.0 \
  --title "Micromegas v0.24.0" \
  --notes "See CHANGELOG.md for details" \
  grafana/micromegas-micromegas-datasource.zip
```

### Phase 4: Post-Release Version Bump to 0.25.0

#### Rust
- `rust/Cargo.toml`: workspace version and all dependency versions to 0.25.0
- `rust/tracing/Cargo.toml`: proc-macros dep to `^0.25`
- `rust/transit/Cargo.toml`: derive-transit dep to `^0.25`
- `rust/datafusion-wasm/Cargo.toml`: version to 0.25.0, all micromegas deps to `^0.25`

#### Other packages
- `python/micromegas/pyproject.toml`: 0.25.0
- `grafana/package.json`: 0.25.0
- `analytics-web-app/package.json`: 0.25.0

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
