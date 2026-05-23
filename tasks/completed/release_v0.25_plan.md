# Release Plan: Micromegas v0.25.0

## Overview

Release version 0.25.0 of Micromegas. Highlights: native OTLP/HTTP ingestion for logs/metrics/traces (`otel_logs_block_processor`, `otel_metrics_block_processor`, `otel_spans` JIT view), map notebook cell with GLB models / native UE coordinates / primitive overlays / object-store-backed `/api/maps`, admin Maps management UI, `net_spans` JIT view with bandwidth flame chart, color/math UDFs (`rgba`, `lerp_color`, `color_scale`, `bin_center`, `lerp`, `unlerp`), HTTP gateway `/gateway/health` liveness endpoint, Unreal net trace instrumentation, React 19 / R3F 9 / drei 10 / RTL 16 upgrade, Yarn 1 → Yarn 4 migration, dev-worker ephemeral runner mode with persistent caches, and 20+ Dependabot security updates. 44 commits since v0.24.0.

## Current Status

- **Version**: 0.25.0 (already bumped during v0.24.0 post-release)
- **Last Release**: v0.24.0 (April 17, 2026)
- **Branch**: release
- **Commits since v0.24.0**: 44

## New Crates in release.py

- **`micromegas-otel-ingestion`** — new library crate added since v0.24.0 (OTLP/HTTP adapter). Optional dep of the public `micromegas` crate; must publish before `micromegas`. Depends on `micromegas-ingestion`, `micromegas-telemetry`, `micromegas-tracing`. Added as Layer 6.5 in `build/release.py`.

## Pre-Release Checklist

### 0. Verify release.py

- [x] Add `micromegas-otel-ingestion` after `micromegas-telemetry-sink`, before `micromegas-perfetto`

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

- [x] `rust/Cargo.toml` workspace version = 0.25.0
- [x] `rust/datafusion-wasm/Cargo.toml` version = 0.25.0
- [x] `python/micromegas/pyproject.toml` version = 0.25.0
- [x] `grafana/package.json` version = 0.25.0
- [x] `analytics-web-app/package.json` version = 0.25.0

### 3. Documentation Updates

- [ ] Update `CHANGELOG.md` — move Unreleased to `## May 2026 - v0.25.0`
- [ ] Update `grafana/CHANGELOG.md` with version sync entry
- [ ] Update `README.md` roadmap with v0.25.0 highlights

### 4. Grafana Plugin Preparation

- [ ] Build plugin archive: `./build-plugin.sh` (from `grafana/`)

### 5. Git Preparation

- [ ] Commit changelog and doc updates
- [ ] Create release tag: `git tag v0.25.0`
- [ ] Create grafana tag: `git tag grafana-v0.25.0`
- [ ] Push: `git push origin release && git push origin v0.25.0 grafana-v0.25.0`

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
gh release create v0.25.0 \
  --title "Micromegas v0.25.0" \
  --notes "See CHANGELOG.md for details" \
  grafana/micromegas-micromegas-datasource.zip
```

### Phase 4: Post-Release Version Bump to 0.26.0

#### Rust
- `rust/Cargo.toml`: workspace version and all dependency versions to 0.26.0
- `rust/tracing/Cargo.toml`: proc-macros dep to `^0.26`
- `rust/transit/Cargo.toml`: derive-transit dep to `^0.26`
- `rust/datafusion-wasm/Cargo.toml`: version to 0.26.0, all micromegas deps to `^0.26`

#### Other packages
- `python/micromegas/pyproject.toml`: 0.26.0
- `grafana/package.json`: 0.26.0
- `analytics-web-app/package.json`: 0.26.0

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
