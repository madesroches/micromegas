# Release Plan: Micromegas v0.29.0

## Overview

Release version 0.29.0 of Micromegas. This is an auth-hardening + query-observability release. Highlights:

- **DB-backed API key store** — new `ingestion_api_keys`/`analytics_api_keys` tables (schema v5) hold only a SHA-256 hash plus a full audit trail, validated through a short-TTL cache; mint/list/revoke/import HTTP routes for both key types now live entirely on `analytics-web-srv` (ingestion itself exposes no key-management surface). A DB outage now surfaces as a retryable 503, not a rejected credential. **Client-visible breaking change**: a key valid on both ingestion and flight-sql today must become two distinct keys after migrating (#1383, #1411, #1458).
- **FlightSQL query audit hardening** — query failures now classify into distinct gRPC status codes (`InvalidArgument`/`ResourceExhausted`/`Unimplemented`/`Internal`) instead of always `Internal`; the audit log gains per-query peak memory/spill attribution, client attribution headers (`x-client-agent`/`-entrypoint`/`-session`), originating notebook/cell, and a `client_ip` fix for the ALB-appended `X-Forwarded-For` case (#1435, #1406, #1436, #1437, #1459). `QueryAuditRecord` (published API) gains several new fields — minor breaking change for any downstream struct literal.
- **`parse_block` decodes OTLP blocks** — a format→decoder registry means `parse_block` now walks OTLP logs/metrics/traces payloads (not just `micromegas-transit` ones), closing a real diagnostic gap for stalled OTLP pipelines (#1467).
- **JIT partition event-time ordering fix** — `thread_spans`/`net_spans` JIT partitions are now cut in event-time order instead of registration order, fixing fragmented call trees for streams whose blocks arrive out of event-time order; `SCHEMA_VERSION` bump triggers a one-off rebuild per stream on first query after deploy (#1429).
- **Ingestion write-path hardening** — block payload object writes are now create-only (`PutMode::Create`) instead of unconditional overwrite, closing the OTLP-redelivery regression from #1462; `BlockPayload` dependencies/objects encode as CBOR byte strings, cutting stored payload size ~40-45% for new blocks (#1465, #1463).
- **Python client** — removed the deprecated `MICROMEGAS_PYTHON_MODULE_WRAPPER` escape hatch (breaking change, OIDC replaces it); AWS-CLI-style named connection profiles in `~/.micromegas/config.json`; `--version` on all three console scripts (#1408, #1403, #1416).
- **Web app** — Pie Chart notebook cell; Arrow `Utf8View`/`BinaryView` decode fix (was breaking any query using `LEFT`/`replace`/etc.); chart axis fixes (non-finite values, long categorical labels); tab favicon execution-state indicator; sidebar flyout-collapse fix (#1339, #1294, #1424, #1425, #1443, #1439).
- **Security** — `undici` 8.10.0, `cryptography` 50.0.0 (CVE-2026-69247), `event-listener` 5.4.2 (RUSTSEC-2026-0221), `js-yaml` 4.3.1, `nanoid` ^3.3.17 — resolving ~15 Dependabot alerts.
- **Build/toolchain** — `analytics-web-app` migrated ESLint 8→10 (flat config) and Tailwind 3→4 (raises browser floor to Safari 16.4+/Chrome 111+/Firefox 128+); React Compiler lint rules enabled at `error`.

40 commits since v0.28.0.

## Current Status

- **Version**: 0.29.0 (already bumped during v0.28.0 post-release — verified across all packages)
- **Last Release**: v0.28.0 (2026-08-02)
- **Branch**: `release`
- **Commits since v0.28.0**: 40

## New Crates & Services Since v0.28.0

Checked `git diff --name-status v0.28.0..HEAD -- rust/` for added `Cargo.toml` files, and diffed the current `rust/*/Cargo.toml` member list against `build/release.py`'s `-p` list and `build/build_docker_images.py`'s `SERVICES` dict.

**Result: no new publishable crate and no new Docker service this cycle.** `release.py`'s crate list and the 7-service `SERVICES` dict (`ingestion`, `flight-sql`, `maintenance`, `object-cache`, `http-gateway`, `analytics-web`, `monolith`) both already match the workspace.

## Pre-Release Checklist

### 0. Fix release.py (if new crates or services were added)

- [x] Verified no new published crates are missing from `build/release.py`
- [x] Verified no new crates in the wasm workspace
- [x] Verified no new server binary needs adding to `SERVICES` in `build/build_docker_images.py`

### 1. Code Quality & Testing

#### Rust Workspace (from `rust/` directory)
- [x] Run full CI pipeline: `python3 ../build/rust_ci.py` (native + WASM, fmt, clippy, tests, `cargo audit`/`cargo deny`) — all green

#### Python Package (from `python/micromegas/` directory)
- [x] `poetry run black . --check` — clean, 60 files unchanged
- [x] `poetry run pytest` — 209 passed, 72 failed (all connection-refused to a non-running server, expected), 6 skipped

#### Grafana Plugin (from `grafana/` directory)
- [x] `yarn install`
- [x] `yarn lint:fix`
- [x] `yarn test:ci` — 47/47 passed
- [x] `yarn build` — succeeded (pre-existing `immutable`/react-awesome-query-builder warnings only)

#### Analytics Web App (from `analytics-web-app/` directory)
- [x] `yarn install`
- [x] `yarn lint` — clean, 0 errors (pre-existing fast-refresh warnings only)
- [x] `yarn type-check` — clean
- [x] `yarn test` — 1378/1378 passed
- [x] `yarn build` — succeeded

### 2. Version Verification (all already 0.29.0 — verified)

- [x] `rust/Cargo.toml` workspace version = 0.29.0
- [x] `rust/datafusion-wasm/Cargo.toml` version = 0.29.0
- [x] `python/micromegas/pyproject.toml` version = 0.29.0
- [x] `grafana/package.json` version = 0.29.0
- [x] `analytics-web-app/package.json` version = 0.29.0
- [x] `blender/micromegas_blender/blender_manifest.toml` version = 0.29.0
- [x] `grep -rnE '0\.28' --include=Cargo.toml rust/` — no stale inter-crate pins found

### 3. Documentation Updates

- [x] Review git log: `git log --oneline v0.28.0..HEAD` — 40 commits
- [x] Update `CHANGELOG.md` — renamed `## Unreleased` to `## v0.29.0 - 2026-08-12`, added a fresh empty `## Unreleased` above it
- [x] Update `grafana/CHANGELOG.md` — added a `## 0.29.0 (2026-08-12)` version-sync entry (no plugin-specific changes this cycle, verified via `git diff v0.28.0..HEAD -- grafana/`)
- [x] Update `README.md` "Recent Releases" — added a `### v0.29.0 (August 2026)` block, **and trimmed the section to the last 3 months** (kept v0.29.0/v0.28.0/v0.27.0/v0.26.0 — August/July/June 2026 — dropped v0.22.0 through v0.25.0, March–May 2026). The "For the full history, see CHANGELOG.md" pointer stays.

### 4. Grafana Plugin Preparation

- [x] Build plugin archive: `./build-plugin.sh` (from `grafana/`) → `grafana/micromegas-micromegas-datasource.zip` (version 0.29.0 stamped in)

### 5. Git Preparation

All four tags must point to the same release commit (workspace at 0.29.0, before the Phase 4 bump):

- [ ] Commit the changelog/doc updates ("Release v0.29.0") — **must include this plan doc itself**, since `cargo release` rejects untracked files (see template lesson from v0.28.0)
- [ ] Create release tags:
  ```bash
  git tag v0.29.0 grafana-v0.29.0 capi-v0.29.0 blender-v0.29.0
  ```
- [ ] Push release branch and all tags:
  ```bash
  git push origin release && git push origin v0.29.0 grafana-v0.29.0 capi-v0.29.0 blender-v0.29.0
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
gh release create v0.29.0 \
  --title "Micromegas v0.29.0 - DB-backed API keys + query audit hardening" \
  --notes "..." \
  grafana/micromegas-micromegas-datasource.zip
```

### Phase 3.5: Docker Images (run BEFORE Phase 4)

```bash
python3 build/build_docker_images.py \
  ingestion flight-sql maintenance object-cache http-gateway analytics-web monolith \
  --all-arches --push --version 0.29.0
```

Verify both platforms pushed:
```bash
docker buildx imagetools inspect marcantoinedesroches/micromegas-monolith:0.29.0
```

### Phase 4: Post-Release Version Bump to 0.30.0

> **WARNING**: Do not start until Phase 1 (all Rust publishes) is complete.

- `rust/Cargo.toml`: workspace version → 0.30.0; all `micromegas-*` path-dep versions → 0.30.0
- `rust/tracing/Cargo.toml`: proc-macros dep → `^0.30`
- `rust/transit/Cargo.toml`: derive-transit dep → `^0.30`
- `rust/datafusion-wasm/Cargo.toml`: version → 0.30.0, all micromegas deps → `^0.30`
- `python/micromegas/pyproject.toml` → 0.30.0
- `grafana/package.json` → 0.30.0
- `analytics-web-app/package.json` → 0.30.0
- `blender/micromegas_blender/blender_manifest.toml` → 0.30.0
- Lock files: `cargo update` (from `rust/`), `yarn install` (grafana + analytics-web-app), `python3 build.py --test` (from `rust/datafusion-wasm/`)
- Commit the bump on the `release` branch

### Phase 5: Cleanup

- [ ] Move this plan from `tasks/` to `tasks/completed/release_v0.29_plan.md`
- [ ] Update `tasks/release_plan_template.md` "Lessons Learned" with anything new this cycle

### Phase 6: Merge to Main

- [ ] Open PR from `release` → `main`
- [ ] Merge after review

## Rollback Plan

- Yank problematic Rust crates: `cargo yank --vers 0.29.0 <crate-name>`
- Update GitHub release notes with issue documentation
- Prepare patch release v0.29.1 if critical issues found

## Decisions

1. **Release date — 2026-08-12.**
2. **Release tagline — "DB-backed API keys + FlightSQL query audit hardening."**
3. **No new crates/services this cycle** — `release.py` and `SERVICES` unchanged.
4. **README "Recent Releases" trimmed to a rolling 3-month window** starting this release (user request 2026-08-12) — going forward, each release's doc-update step drops entries older than 3 months instead of letting the section grow unbounded; full history stays in `CHANGELOG.md`.
