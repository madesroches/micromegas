# Release Plan: Micromegas v0.31.0

## Overview

Release version 0.31.0 of Micromegas. Where v0.30.0 built the audience-based access control
machinery, **v0.31.0 consolidates it**: the env-var configuration surfaces that AbAC temporarily
shadowed are removed outright, Postgres becomes the single source of truth for both API keys and
audience grants, and the v0.30.0 upgrade shims that refused removed variables at startup are
retired on schedule. The second theme is the public website, which was structurally invisible to
crawlers and is now prerendered, canonicalized, and self-checking in CI. Highlights:

- **Postgres is the sole source of API keys and audience grants** (#1502) — `ProviderBuilder` no
  longer reads `{prefix}_API_KEYS`/`MICROMEGAS_API_KEYS` into an env keyring ahead of OIDC and the
  DB key store, and `AudienceReadPolicy`/`AudienceMintPolicy` no longer resolve
  `{prefix}_AUDIENCE_GRANTS`. `ingestion_api_keys`/`analytics_api_keys` and `audience_grants` are
  the only sources, managed through `micromegas-import-keys`, `micromegas-grants`, and
  `analytics-web-srv`'s admin/mint routes. A still-set variable logs a `warn!` naming its
  replacement CLI — a v0.31.0 shim, due for removal in v0.32.0. `object-cache-srv`'s own keyring is
  unaffected and stays permanent by design.
- **v0.30.0 upgrade shims retired** (#1561/#1564) — the startup refusals for the `MICROMEGAS_ADMINS`
  family and the renamed per-role cache-TTL vars are gone; one release later those are just unread
  env vars. `MICROMEGAS_ANALYTICS_PUBLIC_VIEW_SETS` is removed in favor of
  `MICROMEGAS_PUBLIC_VIEW_SETS`, and `FlightSqlServerBuilder::build_and_serve` now resolves the
  isolation config once ahead of every auth branch — previously the injected-provider branch never
  read the env at all, silently leaving such an embedder with no allowlist and no startup error.
- **Merge-path simplification lands its final step** (#1492) — `SqlBatchView`'s `merger_maker` hook
  and `BatchPartitionMerger` are both removed. The v0.30.0 single-reader merge made
  `BatchPartitionMerger`'s event-time batching redundant; a survey found the hook always `None` and
  the merger with no caller anywhere.
- **Website discoverability** — the `welcome/` landing page is prerendered at build time and
  hydrated on the client, so served HTML carries ~400 words instead of 0 (#1574); `mkdocs`'
  `site_url` is corrected to `https://micromegas.info/docs/`, fixing 92 of 93 canonical tags and
  every sitemap entry (#1575); root `robots.txt`/`sitemap.xml` and a favicon ship for the first
  time; the blog gains an RSS/JSON feed. `build/check_docs_site.py` runs between staging and deploy
  in `publish-docs.yml` and fails the build if any sitemap URL, canonical tag, or feed link stops
  resolving — the check that would have caught the original bug.
- **Python CLI auth** — `micromegas-screens` gains `--profile`/`--no-auth` through a shared
  `cli/web_auth.py::resolve_web_auth` helper (#1572); unresolvable auth is now a named error rather
  than a silent unauthenticated client. `micromegas-setup-telemetry` gains `--user-audience SUFFIX`
  composed from the caller's own mint prefix, so one command works for admin and non-admin alike;
  `--claim` (added in v0.30.0, no real-world adoption) is removed outright (#1571).
- **Packaging** — all 16 published crates gain crates.io `categories` metadata (#1565); previously
  none appeared on any category page or lib.rs browse tree. Every `cargo doc` warning in the
  workspace is fixed (#1580).
- **Security** — `rustls` 0.23.45 for RUSTSEC-2026-0285, plus `js-yaml`, `vitest`/`@vitest/mocker`,
  `grpc`, `fflate`, and `fast-uri` bumps resolving Dependabot alerts 469–479 (11 alerts).

18 commits since v0.30.0.

## Current Status

- **Version**: 0.31.0 (already bumped during v0.30.0 post-release — verified across all packages)
- **Last Release**: v0.30.0 (2026-09-02)
- **Branch**: `release`, identical to `main` and `origin/main` at `eb4edd80b`
- **`origin/release`**: does **not** exist on the remote (`git ls-remote --heads origin` shows only
  `main`, `gh-pages`, and one feature branch). The local `remotes/origin/release` ref is stale from
  the v0.30.0 cycle — run `git remote prune origin` before the Phase 5 push, which then *creates*
  the branch with no force needed.
- **Commits since v0.30.0**: 18

## New Crates & Services Since v0.30.0

Diffed `git ls-tree v0.30.0 rust/` against the working tree, and the current service list against
`build/build_docker_images.py`'s `SERVICES`.

**Result: no new crates and no new services.** `build/release.py` (16 crates) and
`build_docker_images.py` (8 publishable services) both need no change. No new wasm-workspace crate.

Publishable services for Phase 3.5 remain **8**: `ingestion`, `flight-sql`, `maintenance`,
`object-cache`, `http-gateway`, `analytics-web`, `monolith`, `redis-exporter` (`all` is dev/test
only and is not published).

## Pre-Release Checklist

### 0. Fix release.py (if new crates or services were added)

- [x] No new published crate missing from `build/release.py`
- [x] No new crate in the wasm workspace
- [x] No new server binary to add to `SERVICES` in `build/build_docker_images.py`

### 1. Code Quality & Testing

#### Rust Workspace (from `rust/`)
- [x] `python3 ../build/rust_ci.py` (native + WASM: fmt, clippy, machete, `cargo audit`, `cargo deny`, tests) — all 5 native + 5 WASM steps passed

#### Python Package (from `python/micromegas/`)
- [x] `poetry run black . --check` — 73 files unchanged
- [x] `poetry run pytest` — 357 passed, 6 skipped, 82 failed; every failure is
      `FlightUnavailable`/`requests.ConnectionError` from no running server, as expected

#### Grafana Plugin (from `grafana/`)
- [x] `yarn install`
- [x] `yarn lint:fix`
- [x] `yarn test:ci`
- [x] `yarn build`

#### Analytics Web App (from `analytics-web-app/`)
- [x] `yarn install`
- [x] `yarn lint` — 0 errors, 6 warnings
- [x] `yarn type-check`
- [x] `yarn test` — 88 files, 1570 tests passed
- [x] `yarn build`

#### Welcome landing page (from `welcome/`) — **new step this cycle**
Not in the template's checklist, but #1574/#1577/#1579 rewrote this package's build (prerender step,
favicon, robots/sitemap) and a broken prerender fails silently as an empty page for crawlers.
- [x] `yarn install`, `yarn lint`, `yarn build`
- [x] Verified `dist/index.html` is 26 KB / 445 words of prerendered text (was a ~1.3 KB empty
      shell), and that `favicon.svg`, `robots.txt`, `sitemap.xml` are emitted

### 2. Version Verification

All packages already read 0.31.0 from the v0.30.0 post-release bump:
- [x] `rust/Cargo.toml` → 0.31.0
- [x] `rust/datafusion-wasm/Cargo.toml` → 0.31.0
- [x] `python/micromegas/pyproject.toml` → 0.31.0
- [x] `grafana/package.json` → 0.31.0
- [x] `analytics-web-app/package.json` → 0.31.0
- [x] `blender/micromegas_blender/blender_manifest.toml` → 0.31.0

### 3. Documentation Updates

- [x] `CHANGELOG.md`: rename `## Unreleased` (line 5) → `## v0.31.0 - 2026-09-14`. Target the line
      by number — `## Unreleased` appears in body text elsewhere. No `still \`## Unreleased\``
      self-references to scrub this cycle (grep count: 0).
- [x] `grafana/CHANGELOG.md`: add `## 0.31.0 (2026-09-14)` version-sync entry (the `grpc` 1.83.2
      bump for alert 475 is the only plugin-touching change)
- [x] `README.md` "Recent Releases": add the `### v0.31.0 (September 2026)` block. The 3-month
      calendar window at a 2026-09-14 release is July/August/September, so v0.27.0 (July 2026)
      still qualifies — **keep five entries** (v0.27.0 … v0.31.0), dropping nothing. Keep the "For
      the full history, see CHANGELOG.md" pointer.

### 4. Grafana Plugin Preparation

- [x] `./build-plugin.sh` from `grafana/` → `grafana/micromegas-micromegas-datasource.zip`

### 5. Git Preparation

All four tags must point at the same release commit (workspace at 0.31.0, before the Phase 4 bump).

- [x] Commit changelog + doc updates **and this plan file** (`cargo release` rejects any untracked
      file, not just dirty tracked ones)
- [x] Create tags one at a time — `git tag A B C` does *not* create three tags:
      ```bash
      for t in v0.31.0 grafana-v0.31.0 capi-v0.31.0 blender-v0.31.0; do git tag "$t"; done
      ```
- [x] `git remote prune origin`, then `git push origin release` (creates the branch) — **requires
      explicit user instruction**
- [ ] Push tags in **two** commands — more than three tags in one push suppresses every tag event,
      so `capi-release.yml`/`blender-extension.yml` would never fire — **requires explicit user
      instruction**:
      ```bash
      git push origin v0.31.0 grafana-v0.31.0
      git push origin capi-v0.31.0 blender-v0.31.0
      ```
- [ ] Confirm both tag workflows started: `gh run list --limit 5`

## Release Process

### Phase 1: Rust Crates Release

```bash
cd /home/mad/micromegas/build && python3 release.py
```

16 crates in dependency order, 60s grace between publishes. On a mid-run failure, resume with the
remaining crates individually:
```bash
cd /home/mad/micromegas/rust && PUBLISH_GRACE_SLEEP=60 cargo release -p <crate> -x --no-confirm
cd /home/mad/micromegas/rust/datafusion-wasm && PUBLISH_GRACE_SLEEP=60 cargo release -p micromegas-datafusion-wasm -x --no-confirm
```

### Phase 2: Python Library Release

```bash
cd python/micromegas && poetry build && poetry publish
```

### Phase 3: GitHub Release + Grafana Plugin

The `grafana-v0.31.0` tag fires no workflow; attach the locally built archive:
```bash
gh release create v0.31.0 \
  --title "Micromegas v0.31.0 - Postgres-Only Auth Config" \
  --notes "..." \
  grafana/micromegas-micromegas-datasource.zip

# capi-/blender- releases land after this one and steal the "Latest" badge
gh release edit v0.31.0 --latest
```

If a tag push fired no workflow, recover without deleting tags — a `workflow_dispatch` at a *tag
ref* satisfies the workflows' `startsWith(github.ref, 'refs/tags/capi-v')` gate:
```bash
gh workflow run capi-release.yml --ref capi-v0.31.0
gh workflow run blender-extension.yml --ref blender-v0.31.0
```

### Phase 3.5: Docker Images — run BEFORE Phase 4, concurrently with Phase 1

`build_docker_images.py` reads the version from `rust/Cargo.toml`; running it after the Phase 4 bump
would tag images `0.32.0`. It shares no inputs with Phase 1 or 2, so it can run alongside them — the
only cost is CPU contention.

One-time setup, re-checked each cycle (binfmt does not survive every WSL restart; the arm64 runtime
stage runs `apt-get` on the target platform even though the builder stages cross-compile):
```bash
docker buildx inspect | grep -i platforms   # must list linux/arm64
docker run --privileged --rm tonistiigi/binfmt --install arm64
```

```bash
python3 build/build_docker_images.py \
  ingestion flight-sql maintenance object-cache http-gateway analytics-web monolith redis-exporter \
  --all-arches --push --version 0.31.0
```

Budget **~2h45m wall-clock** for 8 services × 2 arches; `analytics-web` and `monolith` are the
heaviest and take roughly the last hour between them. Builds are idempotent (buildx layer cache), so
a WSL-sleep interruption is recoverable by re-running. Two verification traps:
- The script's `Building <service>` headers are block-buffered when redirected while buildx streams
  to stderr — for liveness use log mtime, for progress
  `grep -o 'load build definition from [a-z-]*\.Dockerfile' <log> | uniq -c`.
- A `:0.31.0` tag may already exist on Docker Hub from a dev-time `--push` during the cycle (the
  workspace has read 0.31.0 since the v0.30.0 post-release bump). Presence of a tag is not evidence
  the release run reached that service — check the run's own BUILD SUMMARY.

Verify per service with **both** tags — amd64 publishes `:0.31.0`, arm64 publishes `:0.31.0-arm64`;
there is no fused manifest:
```bash
docker buildx imagetools inspect marcantoinedesroches/micromegas-<svc>:0.31.0
docker buildx imagetools inspect marcantoinedesroches/micromegas-<svc>:0.31.0-arm64
```

### Phase 4: Post-Release Version Bump to 0.32.0

> Do not start until Phase 1 is fully complete — `cargo release` reads the workspace version from
> disk and will commit a premature bump as its own "chore: Release" commit, then fail.

- `rust/Cargo.toml`: workspace version + all micromegas dependency versions → 0.32.0
- `rust/tracing/Cargo.toml`: proc-macros dep → `^0.32`
- `rust/transit/Cargo.toml`: derive-transit dep → `^0.32`
- `rust/monolith/Cargo.toml`: `analytics-web-srv` pin → 0.32.0 (binary crates are not in
  `[workspace.dependencies]`)
- `rust/datafusion-wasm/Cargo.toml`: version → 0.32.0, micromegas deps → `^0.32`
- Verify nothing is left behind: `grep -rnE '0\.31' --include=Cargo.toml rust/`
- `python/micromegas/pyproject.toml`, `grafana/package.json`, `analytics-web-app/package.json` → 0.32.0
- `blender/micromegas_blender/blender_manifest.toml` → 0.32.0
- Lock files: `cargo update --workspace` (from `rust/`), `yarn install` (grafana, analytics-web-app),
  `python3 build.py --test` (from `rust/datafusion-wasm/`)
- Commit the bump; push to `release` — **requires explicit user instruction**

### Phase 5: Cleanup

- Move this plan to `tasks/completed/release_v0.31_plan.md`
- Update `tasks/release_plan_template.md` with this cycle's lessons

### Phase 6: Merge to Main

- `git log --oneline main..HEAD`, then open a PR from `release` to `main` — **requires explicit user
  instruction**

## Autonomy Boundaries

Per `CLAUDE.md`, nothing is pushed or published outward without a direct instruction. Executed
autonomously: checklist verification, tests/lints/builds, doc updates, local commits, local tags,
the local Grafana archive build, and the plan/template updates. **Held for explicit approval**:
`git push` (branch and tags), `python3 build/release.py` (crates.io), `poetry publish` (PyPI),
`gh release create`, and `build_docker_images.py --push` (Docker Hub).

## Rollback Plan

- Yank a bad crate: `cargo yank --vers 0.31.0 <crate-name>`
- Document the issue in the GitHub release notes
- Cut v0.31.1 if critical
