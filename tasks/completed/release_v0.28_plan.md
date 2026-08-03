# Release Plan: Micromegas v0.28.0

## Overview

Release version 0.28.0 of Micromegas. This is an access-control + ingestion-surface release. Highlights:

- **Audience-based Access Control** — the five mutating lakehouse SQL functions (`retire_partitions`, `materialize_partitions`, `regenerate_partitions`, `retire_partition_by_file`, `retire_partition_by_metadata`) are now gated on the caller's admin status via a new `x-auth-is-admin` gRPC header; only OIDC callers matched against `MICROMEGAS_ADMINS`/`MICROMEGAS_ANALYTICS_ADMINS` can call them (breaking API: `is_admin: bool` required param threaded through `query`/`make_session_context`/etc.) (#1384).
- **CloudWatch Firehose ingestion** — new `POST /ingestion/otlp/v1/metrics/firehose` and `.../logs/firehose/cloudwatch` endpoints let CloudWatch Metric Streams / CloudWatch Logs reach micromegas via Kinesis Data Firehose HTTP Endpoint Delivery with no Lambda/collector in between; metrics are partitioned per CloudWatch namespace and length-delimited record batches are decoded correctly (#1299, #1381, #1387, #1300, #1388).
- **Lakehouse merge ordering** — order-preserving k-way merge for `SqlBatchView`s and `blocks_view` via a new `ScanOrdering::PerFile` scan mode (breaking API: `View::get_scan_output_ordering`, `PartitionedTableProvider::with_scan_ordering`, `QueryMerger::with_merge_scan_ordering`, `fetch_sql_partition_spec` signatures changed) (#1392, #1336).
- **Python 3.11 minimum + RFC 3339 timestamps** — breaking change raising the minimum supported Python from 3.10 to 3.11, enabling native `Z`-suffixed RFC 3339 parsing across the FlightSQL client and `micromegas-query` CLI (#1405).
- **Screens folders** — folder organization for saved screens (list/create/rename/move/delete), sidebar tree, Save dialog picker, TOCTOU-safe via Postgres advisory locks (#1159).
- **Observability** — per-query FlightSQL audit log (`flightsql_query_audit`), `pg_stat_*` self-observability collector on the maintenance daemon, process RSS/jemalloc gauges on every service, materialization-pass failure isolation so one broken view no longer starves the rest (#1288, #1292, #1319, #1393).
- **Object-cache hardening** — client-side circuit breaker, two-step read replacing a foyer race, disk→RAM promotion metrics, graceful-shutdown drain fixes (#1380, #1360, #1318, #1291).
- **Dependencies/toolchain** — Rust 1.97.1, DataFusion 54.1, Grafana plugin SDK 12.4.6, `analytics-web-app` Jest→Vitest migration, assorted Dependabot fixes.

73 commits since v0.27.0.

## Current Status

- **Version**: 0.28.0 (already bumped during v0.27.0 post-release — verified across all packages)
- **Last Release**: v0.27.0 (2026-07-12)
- **Branch**: `release`
- **Commits since v0.27.0**: 73

## New Crates & Services Since v0.27.0

Checked `git diff --name-status v0.27.0..HEAD -- rust/` for added `Cargo.toml` files and diffed the current `rust/*/Cargo.toml` member list against `build/release.py`'s `-p` list and `build/build_docker_images.py`'s `SERVICES` dict.

**Result: no new publishable crate and no new Docker service this cycle.** `release.py`'s 16-crate dependency-ordered list and the 7-service `SERVICES` dict (`ingestion`, `flight-sql`, `maintenance`, `object-cache`, `http-gateway`, `analytics-web`, `monolith`) both already match the workspace. `rust/public/Cargo.toml` gained a `jemalloc-metrics` feature and new dev-dependencies/tests, but no new crate directory.

## Pre-Release Checklist

### 0. Fix release.py (if new crates or services were added)

- [x] Verified no new published crates are missing from `build/release.py`
- [x] Verified no new crates in the wasm workspace
- [x] Verified no new server binary needs adding to `SERVICES` in `build/build_docker_images.py`

### 1. Code Quality & Testing

#### Rust Workspace (from `rust/` directory)
- [x] Run full CI pipeline: `python3 ../build/rust_ci.py` (native + WASM, fmt, clippy, tests, `cargo audit`/`cargo deny`) — all green

#### Python Package (from `python/micromegas/` directory)
- [x] `poetry run black . --check` — clean
- [x] `poetry run pytest` — 112 passed, 71 failed (all connection-refused to analytics/ingestion servers not running locally, expected), 6 skipped

#### Grafana Plugin (from `grafana/` directory)
- [x] `yarn install`
- [x] `yarn lint:fix`
- [x] `yarn test:ci` — 47/47 passed
- [x] `yarn build` — succeeded (pre-existing `immutable`/react-awesome-query-builder warnings only)

#### Analytics Web App (from `analytics-web-app/` directory)
- [x] `yarn install`
- [x] `yarn lint` — clean (pre-existing fast-refresh warnings only)
- [x] `yarn type-check`
- [x] `yarn test` — 1244/1244 passed
- [x] `yarn build`

### 2. Version Verification (all should already be 0.28.0)

- [x] `rust/Cargo.toml` workspace version = 0.28.0
- [x] `rust/datafusion-wasm/Cargo.toml` version = 0.28.0
- [x] `python/micromegas/pyproject.toml` version = 0.28.0
- [x] `grafana/package.json` version = 0.28.0
- [x] `analytics-web-app/package.json` version = 0.28.0
- [x] `blender/micromegas_blender/blender_manifest.toml` version = 0.28.0

### 3. Documentation Updates

- [x] Review git log: `git log --oneline v0.27.0..HEAD`
- [x] Update `CHANGELOG.md` — renamed `## Unreleased` to `## v0.28.0 - 2026-08-02`, added a fresh empty `## Unreleased` above it
- [x] Update `grafana/CHANGELOG.md` — added a `## 0.28.0 (2026-08-02)` version-sync entry
- [x] Update `README.md` roadmap — added a `### v0.28.0 (August 2026)` block under "Recent Releases"

### 4. Grafana Plugin Preparation

- [x] Build plugin archive: `./build-plugin.sh` (from `grafana/`) → `grafana/micromegas-micromegas-datasource.zip`

### 5. Git Preparation

All four tags must point to the same release commit (workspace at 0.28.0, before the Phase 4 bump):

- [x] Commit the changelog/doc updates ("Release v0.28.0")
- [x] Create release tags:
  ```bash
  git tag v0.28.0 grafana-v0.28.0 capi-v0.28.0 blender-v0.28.0
  ```
- [x] Push release branch and all tags:
  ```bash
  git push origin release && git push origin v0.28.0 grafana-v0.28.0 capi-v0.28.0 blender-v0.28.0
  ```

## Release Process

### Phase 1: Rust Crates Release

```bash
cd /home/mad/micromegas/build
python3 release.py
```

### Phase 2: Python Library Release

From `python/micromegas/`:
```bash
poetry build
poetry publish
```

### Phase 3: Grafana Plugin Release

```bash
gh release create v0.28.0 \
  --title "Micromegas v0.28.0 - <tagline>" \
  --notes "..." \
  grafana/micromegas-micromegas-datasource.zip
```

### Phase 3.5: Docker Images (run BEFORE Phase 4)

```bash
python3 build/build_docker_images.py \
  ingestion flight-sql maintenance object-cache http-gateway analytics-web monolith \
  --all-arches --push --version 0.28.0
```

Verify both platforms pushed:
```bash
docker buildx imagetools inspect marcantoinedesroches/micromegas-monolith:0.28.0
```

### Phase 4: Post-Release Version Bump to 0.29.0

> **WARNING**: Do not start until Phase 1 (all Rust publishes) is complete.

- `rust/Cargo.toml`: workspace version → 0.29.0; all `micromegas-*` path-dep versions → 0.29.0
- `rust/tracing/Cargo.toml`: proc-macros dep → `^0.29`
- `rust/transit/Cargo.toml`: derive-transit dep → `^0.29`
- `rust/datafusion-wasm/Cargo.toml`: version → 0.29.0, all micromegas deps → `^0.29`
- `python/micromegas/pyproject.toml` → 0.29.0
- `grafana/package.json` → 0.29.0
- `analytics-web-app/package.json` → 0.29.0
- `blender/micromegas_blender/blender_manifest.toml` → 0.29.0
- Lock files: `cargo update` (from `rust/`), `yarn install` (grafana + analytics-web-app), `python3 build.py --test` (from `rust/datafusion-wasm/`)
- Commit the bump on the `release` branch

### Phase 5: Cleanup

- [x] Move this plan from `tasks/` to `tasks/completed/release_v0.28_plan.md`
- [x] Update `tasks/release_plan_template.md` "Lessons Learned"

### Phase 6: Merge to Main

- [ ] Open PR from `release` → `main`
- [ ] Merge after review

## Rollback Plan

- Yank problematic Rust crates: `cargo yank --vers 0.28.0 <crate-name>`
- Update GitHub release notes with issue documentation
- Prepare patch release v0.28.1 if critical issues found

## Decisions

1. **Release date — 2026-08-02.**
2. **Release tagline — "Audience-based access control + CloudWatch Firehose ingestion."**
3. **No new crates/services this cycle** — `release.py` and `SERVICES` unchanged.
