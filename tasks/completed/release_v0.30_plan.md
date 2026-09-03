# Release Plan: Micromegas v0.30.0

## Overview

Release version 0.30.0 of Micromegas. This is the **audience-based access control (AbAC) release** — the epic tracked at #1334 lands end to end, from what an audience *is* through write-side stamping, read-side enforcement, a DB-backed grant store, self-service minting, and local groups in Postgres. Highlights:

- **AbAC Query Enforcement** — `OwnershipRewrite` (#1370), a mandatory `AnalyzerRule` injecting an audience-filtering predicate into every `MaterializedView`-backed plan, plus `AudienceGuard` (#1371), arg-addressed guards for the four surfaces the rewrite structurally cannot reach (`process_spans`, `perfetto_trace_chunks`, `parse_block`, `get_payload`) and row filtering on `list_partitions`. A scan-time audience check on `view_instance(...)` (#1486) stops a caller from triggering JIT materialization work for another audience's instance; #1530 then drops the now-redundant injected predicate for the five `view_instance`-only view sets.
- **Server-side audience stamping** — ingestion stamps `micromegas.audience` from the authenticated credential instead of trusting the client (#1373); reserved `micromegas.*` payload properties are dropped at ingestion; OTLP-derived `process_id`/`block_id` become audience-scoped so two tenants posting identical resources no longer collide. Schema v8 gives `processes`, `streams`, **and** `blocks` a real per-row `audience` column (#1518), closing the cross-audience append gap. `audience` is promoted to a physical column on the six global-instance views (#1482).
- **Grant store, self-service mint, and groups** — a DB-backed `audience_grants` table (schema v7, #1489) makes grants editable without a restart; `list_audience_grants()` (#1510) is registered for every authenticated caller; `POST/DELETE /api/audience-grants` behind a `GrantGate` (#1510); non-admin ingestion-key minting gated on a `mint` grant (#1374); schema v9 seeds `public` read/mint (#1535) and removes the built-in `PUBLIC_AUDIENCE` read grant. Finally, **local group membership and admin-ness move into Postgres** (#1549): `groups`/`group_members` tables (schema v10), transitive membership resolution, a reserved `admins` group, `/api/groups*` routes, a Groups admin page, and a `micromegas-groups` CLI.
- **Operator-facing breaks** — `MICROMEGAS_ADMINS`/`MICROMEGAS_ANALYTICS_ADMINS`/`MICROMEGAS_INGESTION_ADMINS` are removed and **refused at startup**; the IdP `groups` claim is no longer read; `MICROMEGAS_API_KEY_CACHE_TTL_SECONDS` + `MICROMEGAS_AUDIENCE_GRANT_CACHE_TTL_SECONDS` collapse onto `MICROMEGAS_AUTH_CACHE_TTL_SECONDS`; `MICROMEGAS_DEFAULT_KEY_AUDIENCE` → `MICROMEGAS_DEFAULT_AUDIENCE`; `MICROMEGAS_UNSTAMPED_AUDIENCE` fails at startup.
- **Partition regeneration required** — all six global-instance views bump their file-schema hash (#1482/#1518), making pre-existing partitions invisible until regenerated in dependency order (`blocks` first, then `processes`/`streams`/`log_stats`, then `log_entries`/`measures`).
- **Merge-path rework** — every non-ordering-declaring view's merge now scans through one reader per source file group (#1491), collapsing the three-way dispatch to concatenate / sort-merge, making merged row order deterministic and restoring row-group pruning for time-filtered queries.
- **New service: `micromegas-redis-exporter`** — samples a Redis instance over one persistent connection and emits `redis_*` metrics through the standard telemetry sink; ships as its own Docker image. All eight jemalloc-backed binaries also gain a tuned `malloc_conf` with background purging (#1536).
- **Query deny list** (#1488) — admin-managed, Postgres-backed, replica-cached rules evaluated as DataFusion boolean expressions over a fixed match context (including a normalized `sql_hash`), checked before planning.
- **JSONB completion** (#1475) — `jsonb_entries`/`jsonb_elements`/`jsonb_path_elements` scalar UDFs; `jsonb_object_keys` returns plain `List<Utf8>`.
- **JIT/ordering fixes** — density-batched JIT partition generation (#1474); `thread_spans` cross-segment `declared scan ordering violated` fix via `max_sort_key_time` (#1478) plus the `micromegas_tracing` flush-path timestamp alignment behind it; `get_process_thread_list` NULL-property panic fix.
- **`retire_partition_by_metadata()` gains a required fifth argument** `file_schema_hash` (#1540) — the four metadata columns were not a unique key, and the missing hash could silently delete two partitions while orphaning one file.
- **Web app** — histogram columns render as per-row bar charts automatically; markdown-cell Run control; `/admin` and `/admin/ingestion-keys` viewable by every authenticated user with role-filtered content; y-axis clipping and unit-suffix fixes.
- **Telemetry sink** — `MICROMEGAS_PROCESS_PROPERTIES` env-var process properties on every micromegas binary.
- **Unreal** — external-profiler bridge (`MicromegasExternalProfiler`); `telemetry.global_context_print`/`_set_property` console commands.
- **Python** — `micromegas-grants`, `micromegas-setup-telemetry` CLIs; `StaticTokenAuthProvider` + profile `api_key_file`; top-level `connect_with_profile()`.
- **Security** — `postcss-selector-parser`, `qs`, `@humanfs/node`, `browserslist`, `thrift`, `grpc` bumps resolving ~9 Dependabot alerts.

43 commits since v0.29.0.

## Current Status

- **Version**: 0.30.0 (already bumped during v0.29.0 post-release — verified across all packages)
- **Last Release**: v0.29.0 (2026-08-12)
- **Branch**: `release` (identical to `main`; `origin` has no `release` branch — it will be created by the Phase 5 push)
- **Commits since v0.29.0**: 43

## New Crates & Services Since v0.29.0

Diffed `rust/*/` directories at `v0.29.0` against the working tree, and the current crate list against `build/release.py`'s `-p` list and `build/build_docker_images.py`'s `SERVICES` dict.

**Result: one new crate — `rust/redis-exporter` (#1497), a binary service.** Nothing depends on it and it is not published to crates.io, so `build/release.py` needs no change. It is already wired into `SERVICES` (`redis-exporter`) with `docker/redis-exporter.Dockerfile` present, so `build_docker_images.py` needs no change either. No new wasm-workspace crate.

Publishable-service count for Phase 3.5 is now **8**: `ingestion`, `flight-sql`, `maintenance`, `object-cache`, `http-gateway`, `analytics-web`, `monolith`, `redis-exporter` (`all` is dev/test only and is not published).

## Pre-Release Checklist

### 0. Fix release.py (if new crates or services were added)

- [x] Verify no new published crate is missing from `build/release.py`
- [x] Verify no new crate in the wasm workspace
- [x] Verify no new server binary needs adding to `SERVICES` in `build/build_docker_images.py`

### 1. Code Quality & Testing

#### Rust Workspace (from `rust/`)
- [x] `python3 ../build/rust_ci.py` (native + WASM, fmt, clippy, tests, `cargo audit`/`cargo deny`)

#### Python Package (from `python/micromegas/`)
- [x] `poetry run black . --check`
- [x] `poetry run pytest` (integration failures from a non-running server are expected)

#### Grafana Plugin (from `grafana/`)
- [x] `yarn install`
- [x] `yarn lint:fix`
- [x] `yarn test:ci`
- [x] `yarn build`

#### Analytics Web App (from `analytics-web-app/`)
- [x] `yarn install`
- [x] `yarn lint`
- [x] `yarn type-check`
- [x] `yarn test`
- [x] `yarn build`

### 2. Version Verification

All packages should already read 0.30.0 from the v0.29.0 post-release bump:
- [x] `rust/Cargo.toml` → 0.30.0
- [x] `rust/datafusion-wasm/Cargo.toml` → 0.30.0
- [x] `python/micromegas/pyproject.toml` → 0.30.0
- [x] `grafana/package.json` → 0.30.0
- [x] `analytics-web-app/package.json` → 0.30.0
- [x] `blender/micromegas_blender/blender_manifest.toml` → 0.30.0

### 3. Documentation Updates

- [x] `CHANGELOG.md`: rename `## Unreleased` → `## v0.30.0 - 2026-09-02`
- [x] `grafana/CHANGELOG.md`: add `## 0.30.0 (2026-09-02)` version-sync entry
- [x] `README.md` "Recent Releases": add the `### v0.30.0 (September 2026)` block and trim to the last 3 calendar months — keep v0.27.0 (July) through v0.30.0 (September), drop v0.26.0 (June 2026). Keep the "For the full history" pointer.

### 4. Grafana Plugin Preparation

- [x] `./build-plugin.sh` from `grafana/` → `grafana/micromegas-micromegas-datasource.zip`

### 5. Git Preparation

All four tags point at the same release commit (workspace at 0.30.0, before the Phase 4 bump).

- [x] Commit changelog + doc updates **and this plan file** (`cargo release` rejects any untracked file, not just dirty tracked ones)
- [x] Create tags one at a time — `git tag A B C` does *not* create three tags:
  ```bash
  for t in v0.30.0 grafana-v0.30.0 capi-v0.30.0 blender-v0.30.0; do git tag "$t"; done
  ```
- [x] `git push origin release` (creates the branch; `origin/release` does not exist — no force needed) — **requires explicit user instruction**
- [x] `git push origin v0.30.0 grafana-v0.30.0 capi-v0.30.0 blender-v0.30.0` — **requires explicit user instruction**

## Release Process

### Phase 1: Rust Crates Release

```bash
cd /home/mad/micromegas/build && python3 release.py
```

15 crates in dependency order, 60s grace between publishes. On a mid-run failure, resume with the remaining crates individually:
```bash
cd /home/mad/micromegas/rust && PUBLISH_GRACE_SLEEP=60 cargo release -p <crate> -x --no-confirm
cd /home/mad/micromegas/rust/datafusion-wasm && PUBLISH_GRACE_SLEEP=60 cargo release -p micromegas-datafusion-wasm -x --no-confirm
```

### Phase 2: Python Library Release

```bash
cd python/micromegas && poetry build && poetry publish
```

### Phase 3: GitHub Release + Grafana Plugin

The `grafana-v0.30.0` tag fires no workflow; attach the locally built archive:
```bash
gh release create v0.30.0 \
  --title "Micromegas v0.30.0 - Audience-Based Access Control" \
  --notes "..." \
  grafana/micromegas-micromegas-datasource.zip
```
`capi-v0.30.0` and `blender-v0.30.0` trigger `capi-release.yml` / `blender-extension.yml`, which attach their artifacts to their own releases.

### Phase 3.5: Docker Images — run BEFORE Phase 4

`build_docker_images.py` reads the version from `rust/Cargo.toml`; running it after the bump would tag images `0.31.0`.

```bash
python3 build/build_docker_images.py \
  ingestion flight-sql maintenance object-cache http-gateway analytics-web monolith redis-exporter \
  --all-arches --push --version 0.30.0
```

Budget **~2–2.5h wall-clock** for 8 services × 2 arches; `analytics-web` and `monolith` are the heaviest. Builds are idempotent (buildx layer cache), so a WSL-sleep interruption is recoverable by re-running. Verify per service with **both** tags — amd64 publishes `:0.30.0`, arm64 publishes `:0.30.0-arm64`; there is no fused manifest:
```bash
docker buildx imagetools inspect marcantoinedesroches/micromegas-<svc>:0.30.0
docker buildx imagetools inspect marcantoinedesroches/micromegas-<svc>:0.30.0-arm64
```

### Phase 4: Post-Release Version Bump to 0.31.0

> Do not start until Phase 1 is fully complete — `cargo release` reads the workspace version from disk.

- `rust/Cargo.toml`: workspace version + all micromegas dependency versions → 0.31.0
- `rust/tracing/Cargo.toml`: proc-macros dep → `^0.31`
- `rust/transit/Cargo.toml`: derive-transit dep → `^0.31`
- `rust/monolith/Cargo.toml`: `analytics-web-srv` pin → 0.31.0 (binary crates are not in `[workspace.dependencies]`)
- `rust/datafusion-wasm/Cargo.toml`: version → 0.31.0, micromegas deps → `^0.31`
- Verify nothing is left behind: `grep -rnE '0\.30' --include=Cargo.toml rust/`
- `python/micromegas/pyproject.toml`, `grafana/package.json`, `analytics-web-app/package.json` → 0.31.0
- `blender/micromegas_blender/blender_manifest.toml` → 0.31.0
- Lock files: `cargo update --workspace` (from `rust/`), `yarn install` (grafana, analytics-web-app), `python3 build.py --test` (from `rust/datafusion-wasm/`)
- Commit the bump; push to `release` — **requires explicit user instruction**

### Phase 5: Cleanup

- Move this plan to `tasks/completed/release_v0.30_plan.md`
- Update `tasks/release_plan_template.md` with this cycle's lessons

### Phase 6: Merge to Main

- Open a PR from `release` to `main` — **requires explicit user instruction**

## Autonomy Boundaries

Per `CLAUDE.md`, nothing is pushed or published outward without a direct instruction. Executed autonomously: checklist verification, tests/lints/builds, doc updates, local commits, local tags, the local Grafana archive build, and the plan/template updates. **Held for explicit approval**: `git push` (branch and tags), `python3 build/release.py` (crates.io), `poetry publish` (PyPI), `gh release create`, and `build_docker_images.py --push` (Docker Hub).

## Rollback Plan

- Yank a bad crate: `cargo yank --vers 0.30.0 <crate-name>`
- Document the issue in the GitHub release notes
- Cut v0.30.1 if critical
