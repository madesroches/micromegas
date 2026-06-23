# Release Plan: Micromegas v0.26.0

## Overview

Release version 0.26.0 of Micromegas. Highlights: `micromegas-monolith` single-process deployment, image streams (screenshots as telemetry) with Unreal + analytics integration, `#[micromegas_main]` optional arguments, resilient Unreal telemetry sink (retry system, priority queues, idle-aware sampling), ARM64 cross-compilation support in Docker images, deep `/ready` readiness probes, graceful shutdown for all services, jemalloc global allocator, chart threshold indicators, image notebook cell, OTLP/JSON content-type support, batched expiry pipeline, DataFusion 53.1, and 44+ Dependabot security updates. 54 commits since v0.25.0.

## Current Status

- **Version**: 0.26.0 (already bumped during v0.25.0 post-release)
- **Last Release**: v0.25.0 (May 23, 2026)
- **Branch**: release
- **Commits since v0.25.0**: 54

## New Crates in release.py

None — `micromegas-monolith` is a binary-only crate (no lib target), not published to crates.io.

## Pre-Release Checklist

### 0. Verify release.py

- [x] No new publishable crates since v0.25.0 — release.py unchanged

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

- [x] `rust/Cargo.toml` workspace version = 0.26.0
- [x] `rust/datafusion-wasm/Cargo.toml` version = 0.26.0
- [x] `python/micromegas/pyproject.toml` version = 0.26.0
- [x] `grafana/package.json` version = 0.26.0
- [x] `analytics-web-app/package.json` version = 0.26.0

### 3. Documentation Updates

- [ ] Update `CHANGELOG.md` — move Unreleased entries to `## June 2026 - v0.26.0`
- [ ] Update `grafana/CHANGELOG.md` with version sync entry
- [ ] Update `README.md` roadmap: move Unreleased to `### v0.26.0 (June 2026)`

### 4. Grafana Plugin Preparation

- [ ] Build plugin archive: `./build-plugin.sh` (from `grafana/`)

### 5. Git Preparation

- [ ] Commit changelog and doc updates
- [ ] Create release tag: `git tag v0.26.0`
- [ ] Create grafana tag: `git tag grafana-v0.26.0`
- [ ] Push: `git push origin release && git push origin v0.26.0 grafana-v0.26.0`

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
gh release create v0.26.0 \
  --title "Micromegas v0.26.0" \
  --notes "See CHANGELOG.md for details" \
  grafana/micromegas-micromegas-datasource.zip
```

### Phase 4: Post-Release Version Bump to 0.27.0

#### Rust
- `rust/Cargo.toml`: workspace version and all dependency versions to 0.27.0
- `rust/tracing/Cargo.toml`: proc-macros dep to `^0.27`
- `rust/transit/Cargo.toml`: derive-transit dep to `^0.27`
- `rust/datafusion-wasm/Cargo.toml`: version to 0.27.0, all micromegas deps to `^0.27`

#### Other packages
- `python/micromegas/pyproject.toml`: 0.27.0
- `grafana/package.json`: 0.27.0
- `analytics-web-app/package.json`: 0.27.0

#### Lock files
- `cargo update` (from `rust/`)
- `yarn install` (from `grafana/`)
- `yarn install` (from `analytics-web-app/`)
- `cd rust/datafusion-wasm && python3 build.py --test`

- [ ] Commit version bump
- [ ] Push to release branch

### Phase 5: Cleanup

- Move completed release plan from `tasks/` to `tasks/completed/`

### Phase 6: Merge to Main

- [ ] Create PR from release to main
- [ ] Merge after CI passes
