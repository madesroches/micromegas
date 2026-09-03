# Release Plan Template for Micromegas

This template is updated after each release with lessons learned.
Last updated: v0.30.0 (2026-09-02)

---

## Lessons Learned from v0.30.0

### Pushing four tags in one `git push` fires NO tag workflows

`git push origin vX.Y.0 grafana-vX.Y.0 capi-vX.Y.0 blender-vX.Y.0` succeeded and created all four
remote tags, but GitHub created **no** `push` tag events for any of them — it suppresses tag events
once a single push carries more than three tags. `capi-release.yml` and `blender-extension.yml`
never ran, so neither the C API archives nor the Blender extension got a release until it was
noticed. **Push the tags in two commands** (or one at a time):

```bash
git push origin vX.Y.0 grafana-vX.Y.0
git push origin capi-vX.Y.0 blender-vX.Y.0
```

Verify afterwards with `gh run list` that both workflows actually started — a silent no-op looks
exactly like a successful push.

**Recovery without deleting tags**: both workflows gate their release job on
`if: startsWith(github.ref, 'refs/tags/capi-v')` (resp. `blender-v`), and a `workflow_dispatch`
aimed at a *tag ref* satisfies that gate. So `gh workflow run capi-release.yml --ref capi-vX.Y.0`
produces the full release, no tag deletion/re-push needed. (The workflows' own comments say manual
dispatch "does NOT create a GitHub Release" — that is only true when dispatching from a *branch*.)

### `gh release create` does not mark the main release "Latest"

The `capi-vX.Y.0` and `blender-vX.Y.0` releases are created *after* `vX.Y.0` by their workflows, so
GitHub hands the "Latest" badge to whichever landed last — the Blender add-on. Finish Phase 3 with
`gh release edit vX.Y.0 --latest`.

### Rename the section, then scrub `## Unreleased` self-references

Entries amended mid-cycle carry parenthetical `(#NNNN, still \`## Unreleased\`)` markers. Once the
section is renamed to `## vX.Y.0`, those read as stale. Replace `still \`## Unreleased\`` with
`same release` (17 occurrences this cycle) — the meaning survives: the amendment shipped alongside
the entry it amends, so no released version ever had the superseded behavior. Note `## Unreleased`
appears many times in body text, so the heading rename must target the line by number, not by
`str.replace`.

### Docker phase: ~2h45m, and it parallelizes with Phase 1

8 services x 2 arches took 2h45m wall-clock (20:07 -> 22:52), the last hour of it on
`analytics-web`/`monolith`/`redis-exporter`. Phase 1 (crates.io) and Phase 2 (PyPI) do not touch the
same inputs, so **Phase 3.5 can run concurrently with Phase 1** — both read `rust/Cargo.toml` at the
pre-bump version, and the only real cost is CPU contention (the first two Docker services were
visibly slower while `cargo publish`'s verification builds were running). Doing so cut ~45 min off
this cycle. Phase 4 still must wait for *both*.

### `--all-arches` needs `binfmt` even though the Dockerfiles cross-compile

The builder stages are pinned to `--platform=$BUILDPLATFORM`, but the final runtime stage
(`FROM debian:bookworm-slim`) runs `apt-get install ca-certificates` on the *target* platform, so
arm64 still needs QEMU. `docker buildx inspect` listing only `linux/amd64*` and `linux/386` is the
tell. Run `docker run --privileged --rm tonistiigi/binfmt --install arm64` before Phase 3.5 — it
does not survive every WSL restart, so check rather than assume it is still registered.

### A Docker image tag may already exist before the release run

`micromegas-redis-exporter:0.30.0` was already on Docker Hub before Phase 3.5 started — the
workspace has read the release version since the *previous* cycle's post-release bump, so any
dev-time `build_docker_images.py --push` during the cycle publishes under it. Presence of a tag is
therefore not evidence that Phase 3.5 reached that service; check the run's own BUILD SUMMARY.
The release run overwrites it, so this is a verification trap, not a correctness problem.

### `build_docker_images.py` stdout is block-buffered when redirected

Its `Building <service>` headers are Python `print()` on stdout; buildx writes to stderr. Redirected
to a file, the headers arrive in ~4KB bursts (or at exit) while buildx output streams live — so a
log tail looks like it is on service 1 long after it has moved on, and a burst of headers looks like
completion. For liveness use log mtime; for progress, `grep -o 'load build definition from
[a-z-]*\.Dockerfile' <log> | uniq -c`.

### New binary-only crate needs no `release.py` change

`rust/redis-exporter` was new this cycle (#1497). Nothing depends on it and it is not published to
crates.io, so only `build_docker_images.py`'s `SERVICES` mattered — and #1497 had already added it
along with `docker/redis-exporter.Dockerfile`. The Phase 3.5 service list is now **8**.

### README 3-month trim is by calendar month

At a 2026-09-02 release the window is July/August/September, so v0.26.0 (2026-06-23) is dropped even
though it is younger than 90 days. This matches what v0.29.0 did (it dropped v0.25.0, 2026-05-23, at
an Aug 12 release). Keep four entries, not five.

---

## Lessons Learned from v0.29.0

### README "Recent Releases" is now a rolling 3-month window, not an ever-growing list

Starting this cycle, the README doc-update step (Pre-Release Checklist §3) trims `README.md`'s "Recent Releases" section to the last 3 months of dated entries every release, not just appends the new one. This cycle that meant dropping v0.22.0 through v0.25.0 (March–May 2026) and keeping v0.26.0–v0.29.0 (June–August 2026). The "For the full history, see CHANGELOG.md" pointer stays untouched — full history was never lost, just no longer duplicated in the README. Do this trim every cycle going forward.

### A fully clean cycle end-to-end

No new crates/services, no test failures, no `cargo release` fallback needed, no stale version-pin greps found anything, Docker build finished in-band without a WSL-sleep interruption. Nothing new to fix in the process itself this time beyond the README change above — recorded here mainly so a future reader doesn't wonder whether this cycle was skipped.

---

## Lessons Learned from v0.28.0

### `cargo release` refuses to run with ANY untracked file, not just dirty tracked files

Phase 1 failed immediately on the first crate (`micromegas-derive-transit`) with "uncommitted changes detected" because the freshly-written `tasks/release_v0.28_plan.md` was untracked — it had never been committed. `cargo release`'s clean-tree check rejects untracked files just like modified ones. **Commit the release plan doc itself (along with the changelog/doc updates) before running Phase 1**, not just the tracked-file changes.

### Docker phase estimate revised: budget ~2–2.5h, not ~1h

The prior template said "~1h of real compute" for 7 services × 2 arches. This cycle, 5 of 7 services (both arches) were done at 87 minutes elapsed; the last two services (`analytics-web`, `monolith`) are the heaviest — `analytics-web` bundles the wasm/frontend build on top of the Rust binary, and `monolith` links every service's code into one binary — so they take noticeably longer per-arch than the thin single-binary services. Total wall-clock this cycle was well over 2 hours. Plan accordingly when scheduling this phase; it is not a "run it and check back in an hour" step.

---

## Lessons Learned from v0.27.0

### New lib crate depended on by published crates → add to release.py before its dependents

`micromegas-object-cache` was new this cycle and is depended on by `micromegas-ingestion` and `micromegas-analytics`, but was missing from `build/release.py`. `cargo publish` requires every path-dependency with a version to already exist on crates.io, so the run would have failed at `ingestion`. **Pre-release, diff new `rust/*/Cargo.toml` members against the `release.py` layer list** and insert any new publishable crate before its dependents (object-cache went in as Layer 5.5, after telemetry, before ingestion).

### Keep the Phase 3.5 Docker service list in sync with SERVICES

This cycle: `admin` → `maintenance` rename (#1268) and a new `object-cache` service, for 7 publishable services total. Prefer deriving the list from `SERVICES.keys()` rather than hardcoding.

### `git tag A B C` does NOT create three tags

`git tag v0.27.0 grafana-v0.27.0 capi-v0.27.0 blender-v0.27.0` fails — git reads the 2nd arg as the commit-ish. Create each meta tag in a loop: `for t in ...; do git tag "$t"; done`.

### origin/release may be a STALE local tracking ref

The release branch is re-cut from `main` each cycle and the remote branch is often deleted between releases. A leftover `origin/release` tracking ref made the local branch look "58 ahead / 5 behind" a branch that no longer existed on origin. `git ls-remote --heads origin` showed only `main`/`gh-pages`. Fix: `git remote prune origin`, then a plain `git push origin release` **creates** the branch (no force needed). Verify with `git ls-remote` before assuming divergence or reaching for `--force`.

### Docker phase is long and WSL sleep interrupts it

Building 7 services × 2 arches takes ~1h of real compute. Closing the laptop lid suspends the WSL VM and freezes the build (buildx step timers show huge wall-clock jumps — e.g. a single compile step reading ~30,000 s is the sleep gap, not real work). Builds are idempotent: buildx layer cache means re-running finishes only the remaining services. Use log mtime/advancing timestamps for liveness — `pgrep` gives false negatives between buildx invocations.

### build_docker_images.py publishes ARCH-SUFFIXED tags, not a fused manifest

amd64 → `:X.Y.Z`, arm64 → `:X.Y.Z-arm64`. There is no `manifest create` step, so `:X.Y.Z` is amd64-only by design. Verify completion per service with BOTH `imagetools inspect …:X.Y.Z` and `…:X.Y.Z-arm64`, not by checking platforms inside one manifest.

### The version bump must grep ALL Cargo.toml, not just the workspace root

`rust/monolith/Cargo.toml` pins `analytics-web-srv = { version = "0.27.0" }` directly (binary crates aren't in `[workspace.dependencies]`). Bumping only `rust/Cargo.toml` left a stale `^0.27.0` requirement that broke `cargo update`. Run `grep -rnE '0\.27' --include=Cargo.toml rust/` to catch inter-crate pins. Use `cargo update --workspace` to sync only the micromegas crate versions in Cargo.lock (no third-party churn in the bump commit).

---

## Lessons Learned from v0.26.0

### Do NOT bump versions before Phase 1 completes

`cargo release` reads the workspace `Cargo.toml` from the working tree and commits whatever version it finds as its own "chore: Release" commit. If you bump to X.Z.0 before Phase 1 finishes, `cargo release` will commit the bumped version and then fail when it tries to publish — because `rust/tracing/Cargo.toml` still references `^X.Y` for proc-macros but the local proc-macros crate is now at X.Z.0. **Wait until all crates in Phase 1 are published before doing the Phase 4 version bump.**

If `cargo release` creates a spurious "chore: Release" commit due to a premature bump: `git reset HEAD~1` to undo it, restore Cargo.toml files with `git checkout -- <files>`, and re-run cleanly.

---

## Lessons Learned from v0.21.0

### Publishing New Crates: Version Requirements
When a new crate is added to the workspace that other crates depend on (e.g. `micromegas-datafusion-extensions`), ensure:

1. **release.py order**: The new crate must be published BEFORE any crate that depends on it. `micromegas-datafusion-wasm` was placed in Layer 1 but it depends on `micromegas-datafusion-extensions` (Layer 7.5). Fix: move the dependent crate after all its dependencies.

2. **Version requirements in path deps**: For crates NOT in the main workspace (like `datafusion-wasm` which is in its own Cargo.toml), path dependencies to workspace crates must include `version = "^X.Y"` in addition to `path = "..."`. Without this, `cargo publish` fails with "all dependencies must have a version requirement specified when publishing".

3. **cfg gates for platform-specific calls**: Code in the wasm crate that calls platform-specific APIs must be gated with `#[cfg(target_arch = "wasm32")]`. The `cargo release` verification compiles against native target, so calls to WASM-only functions like `micromegas_telemetry_sink::init_telemetry()` (which only exists in the wasm module) will fail. Gate the call:
   ```rust
   #[cfg(target_arch = "wasm32")]
   {
       let guard = micromegas_telemetry_sink::init_telemetry()...;
       std::mem::forget(guard);
   }
   ```

4. **Build and test WASM after any wasm crate changes**: Always run `python3 build.py --test` from `rust/datafusion-wasm/` after any changes to the wasm crate, not just the initial CI run.

---

## Pre-Release Checklist

### 0. Fix release.py (if new crates or services were added)

- [ ] Verify any new published crates are in `build/release.py` in the correct dependency order
- [ ] Verify new crates in the wasm workspace have explicit `version = "^X.Y"` on all micromegas path deps
- [ ] If a new server binary was added: add it to `SERVICES` in `build/build_docker_images.py` and create its Dockerfile in `docker/`

### 1. Code Quality & Testing

#### Rust Workspace (from `rust/` directory)
- [ ] Run full CI pipeline: `python3 ../build/rust_ci.py` (runs native + WASM CI)
- [ ] **WASM-specific**: If `datafusion-wasm/` was modified, also run `python3 build.py --test` from that directory to confirm WASM tests pass independently

#### Python Package (from `python/micromegas/` directory)
- [ ] Run Python tests: `poetry run pytest` (integration test failures due to missing server are expected)
- [ ] Python code formatting: `poetry run black . --check`

#### Grafana Plugin (from `grafana/` directory)
- [ ] Install dependencies: `yarn install`
- [ ] Run linter: `yarn lint:fix`
- [ ] Run tests: `yarn test:ci`
- [ ] Build plugin: `yarn build`

#### Analytics Web App (from `analytics-web-app/` directory)
- [ ] Install dependencies: `yarn install`
- [ ] Run linter: `yarn lint`
- [ ] Run type check: `yarn type-check`
- [ ] Run tests: `yarn test`
- [ ] Build app: `yarn build`

### 2. Version Verification

All versions should already be at X.Y.0 from the previous post-release bump:
- [ ] Verify workspace version in `rust/Cargo.toml`
- [ ] Verify `rust/datafusion-wasm/Cargo.toml` version
- [ ] Verify Python version in `python/micromegas/pyproject.toml`
- [ ] Verify Grafana plugin version in `grafana/package.json`
- [ ] Verify analytics web app version in `analytics-web-app/package.json`
- [ ] (Optional) Verify `blender/micromegas_blender/blender_manifest.toml` `version` equals X.Y.0 — the released artifact always gets the workspace version stamped in at build time, but a stale hardcoded value mis-labels the version shown in Blender's Extensions UI

### 3. Documentation Updates

- [ ] Review git log: `git log --oneline vX.Y.0..HEAD`
- [ ] Update `CHANGELOG.md` — move Unreleased entries to `## vX.Y.0 - <date>` section
- [ ] Update `grafana/CHANGELOG.md` with version sync entry
- [ ] Update `README.md` "Recent Releases" — add the new `### vX.Y.0` block, **and trim the section to the last 3 months** (drop entries older than 3 months back from the release date). Keep the "For the full history, see CHANGELOG.md" pointer.

### 4. Grafana Plugin Preparation

- [ ] Build plugin archive: `./build-plugin.sh` (from `grafana/` directory)

### 5. Git Preparation

All four tags must point to the **same release commit** (workspace at X.Y.0, before the Phase 4 bump):

- [ ] Commit changelog and doc updates
- [ ] Create release tags: `git tag vX.Y.0 grafana-vX.Y.0 capi-vX.Y.0 blender-vX.Y.0`
  - `vX.Y.0` — main GitHub release (created in Phase 3)
  - `grafana-vX.Y.0` — no tag-triggered workflow; the Grafana archive is built locally and attached to the release in Phase 3
  - `capi-vX.Y.0` — triggers `capi-release.yml`, which builds Linux/Windows C API libs and attaches them to a GitHub Release
  - `blender-vX.Y.0` — triggers `blender-extension.yml`, which zips the Blender extension (version stamped from workspace) and attaches it to a GitHub Release
- [ ] Push release branch: `git push origin release`
- [ ] Push the tags in **two** commands — more than three tags in one push suppresses every tag event, so `capi-release.yml`/`blender-extension.yml` never fire:
  ```bash
  git push origin vX.Y.0 grafana-vX.Y.0
  git push origin capi-vX.Y.0 blender-vX.Y.0
  ```
- [ ] Confirm both tag workflows started: `gh run list --limit 5`

---

## Release Process

### Phase 1: Rust Crates Release

```bash
cd /home/mad/micromegas/build
python3 release.py
```

Crates published in dependency order (60s grace period between publishes).

If `release.py` fails mid-run for already-published crates (their git tags exist), run the remaining crates individually:
```bash
cd /home/mad/micromegas/rust
PUBLISH_GRACE_SLEEP=60 cargo release -p <crate-name> -x --no-confirm

# For the wasm crate (separate workspace):
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

The `grafana-vX.Y.0` tag fires **no** GitHub Actions workflow (the Grafana plugin workflow only triggers on branch/PR events, not tags). Build and attach the archive locally:

```bash
gh release create vX.Y.0 \
  --title "Micromegas vX.Y.0 - <tagline>" \
  --notes "..." \
  grafana/micromegas-micromegas-datasource.zip

# capi-/blender- releases land after this one and steal the "Latest" badge
gh release edit vX.Y.0 --latest
```

### Phase 3.5: Docker Images

> **Run this before Phase 4** — `build_docker_images.py` reads the workspace version from `rust/Cargo.toml`. Running it after the Phase 4 bump would tag images with the next dev version.

One-time setup (if not already done on this machine):

```bash
docker buildx create --use
docker run --privileged --rm tonistiigi/binfmt --install arm64
docker login
```

Publish all 8 services for both architectures (this can run concurrently with Phase 1):

```bash
python3 build/build_docker_images.py \
  ingestion flight-sql maintenance object-cache http-gateway analytics-web monolith redis-exporter \
  --all-arches --push --version X.Y.0
```

Verify both platforms were pushed:

```bash
docker buildx imagetools inspect marcantoinedesroches/micromegas-monolith:X.Y.0
```

Expected output shows both `linux/amd64` and `linux/arm64` platforms.

### Phase 4: Post-Release Version Bump to X.Z.0

> **WARNING**: Do not start this phase until Phase 1 (all Rust crate publishes) is fully complete. `cargo release` reads the workspace version from disk — a premature bump will cause it to commit and then fail mid-run.

#### Rust (`rust/Cargo.toml`):
- Workspace version to X.Z.0
- All dependency versions to X.Z.0
- `rust/tracing/Cargo.toml`: proc-macros dependency to `^X.Z`
- `rust/transit/Cargo.toml`: derive-transit dependency to `^X.Z`
- `rust/datafusion-wasm/Cargo.toml`: version to X.Z.0, all micromegas deps to `^X.Z`

#### Other packages:
- `python/micromegas/pyproject.toml`: version to X.Z.0
- `grafana/package.json`: version to X.Z.0
- `analytics-web-app/package.json`: version to X.Z.0

#### Lock files:
- `cargo update` (from `rust/`)
- `yarn install` (from `grafana/`)
- `yarn install` (from `analytics-web-app/`)
- Rebuild WASM: `python3 build.py --test` (from `rust/datafusion-wasm/`) to update its Cargo.lock

- Commit version bump
- Push to release branch

### Phase 5: Cleanup

- Move completed release plan from `tasks/` to `tasks/completed/`

### Phase 6: Merge to Main

- Create PR from release to main
- Merge after review

---

## Rollback Plan

If issues are discovered after release:
- Yank problematic Rust crates: `cargo yank --vers X.Y.0 <crate-name>`
- Update GitHub release notes with issue documentation
- Prepare patch release vX.Y.1 if critical issues found
