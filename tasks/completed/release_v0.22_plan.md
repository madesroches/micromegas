# Release Plan: Micromegas v0.22.0

## Overview

Release version 0.22.0 of Micromegas. This release features flame graph visualization, async span depth fixes, default system properties, database migration with unique indexes, JSONPath UDFs, row selection in tables, notebook UX improvements (ESC close, Alt+PageUp/PageDown navigation, cell insert above/below, CSV download), and numerous security/dependency updates. 163 commits since v0.21.0.

## Current Status

- **Version**: 0.22.0 (already bumped during v0.21.0 post-release)
- **Last Release**: v0.21.0 (February 27, 2026)
- **Branch**: main (will create release branch)
- **Commits since v0.21.0**: 163

## New Crates in release.py

None. No new published crates added since v0.21.0.

## Pre-Release Checklist

### 0. Verify release.py

- [ ] No new crates to add — confirmed

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

- [ ] `rust/Cargo.toml` workspace version = 0.22.0
- [ ] `rust/datafusion-wasm/Cargo.toml` version = 0.22.0
- [ ] `python/micromegas/pyproject.toml` version = 0.22.0
- [ ] `grafana/package.json` version = 0.22.0
- [ ] `analytics-web-app/package.json` version = 0.22.0

### 3. Documentation Updates

- [ ] Update `CHANGELOG.md` — move Unreleased to `## March 2026 - v0.22.0`
- [ ] Update `grafana/CHANGELOG.md` with version sync entry
- [ ] Update `README.md` roadmap if needed

### 4. Grafana Plugin Preparation

- [ ] Build plugin archive: `./build-plugin.sh` (from `grafana/`)

### 5. Git Preparation

- [ ] Create release branch: `git checkout -b release`
- [ ] Commit changelog and doc updates
- [ ] Create release tag: `git tag v0.22.0`
- [ ] Create grafana tag: `git tag grafana-v0.22.0`
- [ ] Push: `git push origin release && git push origin v0.22.0 grafana-v0.22.0`

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
gh release create v0.22.0 \
  --title "Micromegas v0.22.0" \
  --notes "See CHANGELOG.md for details" \
  grafana/micromegas-micromegas-datasource.zip
```

### Phase 4: Post-Release Version Bump to 0.23.0

#### Rust
- `rust/Cargo.toml`: workspace version and all dependency versions to 0.23.0
- `rust/tracing/Cargo.toml`: proc-macros dep to `^0.23`
- `rust/transit/Cargo.toml`: derive-transit dep to `^0.23`
- `rust/datafusion-wasm/Cargo.toml`: version to 0.23.0, all micromegas deps to `^0.23`

#### Other packages
- `python/micromegas/pyproject.toml`: 0.23.0
- `grafana/package.json`: 0.23.0
- `analytics-web-app/package.json`: 0.23.0

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
