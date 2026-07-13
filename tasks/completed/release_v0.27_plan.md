# Release Plan: Micromegas v0.27.0

## Overview

Release version 0.27.0 of Micromegas. This is a caching + native-integration release. Highlights:

- **Tiered object read cache** — a new standalone range-aware S3 read cache service (`micromegas-object-cache-srv`, Foyer RAM+disk) plus a new in-process **L1** cache, both built on the new `micromegas-object-cache` crate. Includes single-flight coalescing, priority budgeting, memory-bounded prefetch, NDJSON-streamed `/prefetch` ingestion, streamed range responses, write-time cache warming, and extensive performance telemetry.
- **Postgres `partition_metadata` table removed** — partition Parquet metadata now read solely from the Parquet footer through the object-cache-backed reader (schema v6), removing TOAST/write-path overhead.
- **Blender observability add-on + C ABI** — new `micromegas-capi` C-ABI crate (`cdylib`/`staticlib`) and a Blender 4.2+ Python extension (action capture, performance metrics, crash harvester, exception capture), with `capi-release` and `blender-extension` CI workflows.
- **Hardening** — resilient Rust telemetry sink transport (priority queues, retry tuning, in-flight gating), transit block parsing hardened against malformed payloads, and new supply-chain CI gates (`cargo audit` + `cargo deny`).
- **`telemetry-admin` retired** — maintenance daemon binary renamed to `telemetry-maintenance-srv`.
- **Dependencies** — DataFusion 54.0, `syn` 1→2 migration for internal proc-macros, `opentelemetry-proto` 0.32 (GHSA), plus Dependabot fixes.

55 commits since v0.26.0.

## Current Status

- **Version**: 0.27.0 (already bumped during v0.26.0 post-release — verified across all packages)
- **Last Release**: v0.26.0 (June 23, 2026)
- **Branch**: `release`
- **Commits since v0.26.0**: 55

## New Crates & Services Since v0.26.0

| Crate / dir | Kind | crates.io? | Action |
|---|---|---|---|
| `rust/object-cache` (`micromegas-object-cache`) | lib | **YES — publishes by default** | **Must add to `build/release.py`** (see fix below) |
| `rust/object-cache-srv` (`micromegas-object-cache-srv`) | binary/server | No (not a lib dep of any published crate) | Docker image only — already in `SERVICES` as `object-cache` |
| `rust/capi` (`micromegas-capi`) | cdylib/staticlib | **No — decided** | Released via `capi-v0.27.0` tag → `capi-release.yml`; **not a crates.io publish** (see note below) |
| `blender/micromegas_blender` | Python extension | n/a | Released via `blender-v0.27.0` tag → `blender-extension.yml` |

> **Also new but not published to crates.io** (binary-only servers, consistent with prior releases): `monolith`, `http-gateway`, `uri-handler`, `analytics-web-srv`, `telemetry-maintenance-srv` (renamed from admin). These ship as Docker images / GitHub release artifacts, not crates.

> **`micromegas-capi` stays off crates.io (decided).** crates.io serves Rust `cargo add` consumers, but capi's audience is explicitly *non-Rust* (Python/C/C++/game-engine callers) — a Rust project would depend on `micromegas-telemetry-sink`/`micromegas-tracing` directly, never the C-ABI wrapper. That audience is already served by the per-platform prebuilt archives (shared lib + static lib + `micromegas.h`) that the `capi-v*` tag publishes as GitHub Release assets via `capi-release.yml` — the right artifact for a non-Rust consumer, not a source crate. No `publish = false` is needed: like every other non-published crate in the workspace, capi is gated off crates.io solely by omission from `build/release.py`'s explicit per-package `-p <crate>` allow-list, so the documented release flow will never publish it.

## ⚠️ CRITICAL Pre-Release Fix: `micromegas-object-cache` missing from `release.py`

`build/release.py` publishes crates in dependency order but **does not include `micromegas-object-cache`**. This crate:

- Publishes by default (no `publish = false` in `rust/object-cache/Cargo.toml`).
- Is depended on by **`micromegas-ingestion`** (`rust/ingestion/Cargo.toml:13`) and **`micromegas-analytics`** (`rust/analytics/Cargo.toml:15`) via `micromegas-object-cache.workspace = true` (workspace declares `version = "0.27.0"`).
- Depends only on `micromegas-tracing` and `micromegas-transit`.

**If not fixed, `release.py` will fail** at `cargo release -p micromegas-ingestion`, because `cargo publish` requires every path-dependency with a version to already exist on crates.io.

**Fix**: insert `micromegas-object-cache` into `build/release.py` as a new layer **after** telemetry (Layer 5) and **before** ingestion (Layer 6):

```python
# Layer 5.5: Object cache engine (depends on tracing, transit)
run_command("cargo release -p micromegas-object-cache -x --no-confirm")

# Layer 6: Core services (depend on telemetry, tracing, transit, object-cache)
run_command("cargo release -p micromegas-ingestion -x --no-confirm")
```

This must land (committed to the `release` branch) **before** running Phase 1.

## Pre-Release Checklist

### 0. Fix release.py & Docker service list

- [ ] **Add `micromegas-object-cache` to `build/release.py`** at Layer 5.5 (before `micromegas-ingestion`) — see CRITICAL section above
- [ ] Confirm no other new publishable crate is missing: only `object-cache` is a new crates.io publish this cycle
- [ ] `object-cache-srv`, `capi`, and the servers stay out of `release.py` (not crates.io publishes)
- [ ] Docker: `object-cache.Dockerfile` already present and `object-cache` already in `SERVICES` (`build/build_docker_images.py`) — no change needed there, but the **Phase 3.5 publish command must be updated** (see below): `admin` was renamed to `maintenance` (#1268) and `object-cache` is new

### 1. Code Quality & Testing

#### Rust Workspace (from `rust/` directory)
- [ ] Run full CI pipeline: `python3 ../build/rust_ci.py` (native + WASM, format check, clippy, tests, and the new `cargo audit` / `cargo deny` gates)
- [ ] WASM: covered by `rust_ci.py`; if `datafusion-wasm/` changed further, also run `python3 build.py --test` from that directory
- [ ] object-cache: confirm `micromegas-object-cache` builds standalone (`cargo build -p micromegas-object-cache`) since it is now a publish target

#### Python Package (from `python/micromegas/` directory)
- [ ] `poetry run black . --check`
- [ ] `poetry run pytest` (integration failures due to missing server are expected)

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

### 2. Version Verification (all should already be 0.27.0)

- [x] `rust/Cargo.toml` workspace version = 0.27.0
- [x] `rust/datafusion-wasm/Cargo.toml` version = 0.27.0
- [x] `python/micromegas/pyproject.toml` version = 0.27.0
- [x] `grafana/package.json` version = 0.27.0
- [x] `analytics-web-app/package.json` version = 0.27.0
- [x] `blender/micromegas_blender/blender_manifest.toml` version = 0.27.0

### 3. Documentation Updates

- [ ] Review git log: `git log --oneline micromegas-v0.26.0..HEAD`
- [ ] Update `CHANGELOG.md` — the `## Unreleased` section is already comprehensive; rename it to `## v0.27.0 - 2026-07-12` and add a fresh empty `## Unreleased` above it
- [ ] Update `grafana/CHANGELOG.md` — add `## 0.27.0 (2026-07-12)` version-sync entry (note the Dependabot bumps: `golang.org/x/net` 0.55.0)
- [ ] Update `README.md` roadmap — add a `### v0.27.0 (July 2026)` block under "Recent Releases" with the highlights (tiered object cache, partition_metadata removal, Blender/C-ABI integration, sink/transit hardening, supply-chain gates, `telemetry-maintenance-srv` rename, DataFusion 54)

### 4. Grafana Plugin Preparation

- [ ] Build plugin archive: `./build-plugin.sh` (from `grafana/`) → `grafana/micromegas-micromegas-datasource.zip`

### 5. Git Preparation

All four tags must point to the **same release commit** (workspace at 0.27.0, before the Phase 4 bump):

- [ ] Commit the `release.py` fix + changelog/doc updates (e.g. "Release v0.27.0")
- [ ] Create release tags:
  ```bash
  git tag v0.27.0 grafana-v0.27.0 capi-v0.27.0 blender-v0.27.0
  ```
  - `v0.27.0` — main GitHub release (created in Phase 3)
  - `grafana-v0.27.0` — no tag-triggered workflow; archive built locally and attached in Phase 3
  - `capi-v0.27.0` — triggers `capi-release.yml` (Linux/Windows C API libs)
  - `blender-v0.27.0` — triggers `blender-extension.yml` (Blender extension zip, version stamped from workspace)
- [ ] **Do not push yet** — pushing requires explicit user instruction. When authorized:
  ```bash
  git push origin release && git push origin v0.27.0 grafana-v0.27.0 capi-v0.27.0 blender-v0.27.0
  ```

## Release Process

### Phase 1: Rust Crates Release

```bash
cd /home/mad/micromegas/build
python3 release.py
```

Crates publish in dependency order (60s grace between publishes). **Verify `micromegas-object-cache` publishes before `micromegas-ingestion`.**

If `release.py` fails mid-run for already-published crates (their tags exist), resume with the remaining crates individually:
```bash
cd /home/mad/micromegas/rust
PUBLISH_GRACE_SLEEP=60 cargo release -p <crate-name> -x --no-confirm
# wasm crate (separate workspace):
cd /home/mad/micromegas/rust/datafusion-wasm
PUBLISH_GRACE_SLEEP=60 cargo release -p micromegas-datafusion-wasm -x --no-confirm
```

### Phase 2: Python Library Release

From `python/micromegas/`:
```bash
poetry build
poetry publish
```

### Phase 3: Grafana Plugin Release

The `grafana-v0.27.0` tag fires no workflow — build and attach the archive with the GitHub release:
```bash
gh release create v0.27.0 \
  --title "Micromegas v0.27.0 - Tiered object cache + Blender observability" \
  --notes "..." \
  grafana/micromegas-micromegas-datasource.zip
```

### Phase 3.5: Docker Images (run BEFORE Phase 4)

`build_docker_images.py` reads the workspace version from `rust/Cargo.toml`; run it before the Phase 4 bump.

> **Updated service list for v0.27.0**: `admin` → `maintenance` (#1268), and `object-cache` is new. There are now **7** publishable services.

```bash
python3 build/build_docker_images.py \
  ingestion flight-sql maintenance object-cache http-gateway analytics-web monolith \
  --all-arches --push --version 0.27.0
```

Verify both platforms pushed:
```bash
docker buildx imagetools inspect marcantoinedesroches/micromegas-monolith:0.27.0
docker buildx imagetools inspect marcantoinedesroches/micromegas-object-cache:0.27.0
```

### Phase 4: Post-Release Version Bump to 0.28.0

> **WARNING**: Do not start until Phase 1 (all Rust publishes) is complete — `cargo release` reads the on-disk workspace version.

- `rust/Cargo.toml`: workspace version → 0.28.0; all `micromegas-*` path-dep versions → 0.28.0 (**including the new `micromegas-object-cache` / `micromegas-object-cache-srv` entries**)
- `rust/tracing/Cargo.toml`: proc-macros dep → `^0.28`
- `rust/transit/Cargo.toml`: derive-transit dep → `^0.28`
- `rust/datafusion-wasm/Cargo.toml`: version → 0.28.0, all micromegas deps → `^0.28`
- `python/micromegas/pyproject.toml` → 0.28.0
- `grafana/package.json` → 0.28.0
- `analytics-web-app/package.json` → 0.28.0
- `blender/micromegas_blender/blender_manifest.toml` → 0.28.0
- Lock files: `cargo update` (from `rust/`), `yarn install` (grafana + analytics-web-app), `python3 build.py --test` (from `rust/datafusion-wasm/`)
- Commit the bump on the `release` branch

### Phase 5: Cleanup

- Move this plan from `tasks/` to `tasks/completed/release_v0.27_plan.md`
- Update `tasks/release_plan_template.md` "Lessons Learned" with the object-cache/release.py gotcha and the maintenance/object-cache Docker-service additions

### Phase 6: Merge to Main

- Open PR from `release` → `main`
- Merge after review

## Rollback Plan

- Yank problematic Rust crates: `cargo yank --vers 0.27.0 <crate-name>` (note: `micromegas-object-cache` is now yankable and a dependency of ingestion/analytics — yank the dependents too if it is defective)
- Update GitHub release notes with issue documentation
- Prepare patch release v0.27.1 if critical

## Lessons to Carry Forward (for the template)

1. **New lib crate that other published crates depend on → add to `release.py` before its dependents.** `micromegas-object-cache` (new this cycle) is depended on by `ingestion` and `analytics`; missing it breaks the whole publish run. The pre-release "Verify release.py" step must diff new `rust/*/Cargo.toml` members against the `release.py` layer list.
2. **Keep the Phase 3.5 Docker command in sync with `SERVICES`.** This cycle: `admin` → `maintenance` rename and the new `object-cache` service. Prefer deriving the list from `SERVICES.keys()` (minus `all`) rather than hardcoding.

## Decisions

1. **Release date — 2026-07-12.** Used for the `CHANGELOG.md`, `grafana/CHANGELOG.md`, and `README.md` entries.
2. **Release tagline — "Tiered object cache + Blender observability."** Used in the Phase 3 GitHub release title.
3. **`micromegas-capi` stays off crates.io.** Released as prebuilt per-platform archives via the `capi-v0.27.0` tag → `capi-release.yml` only; no crates.io publish. Rationale in the "New Crates & Services" section above.
