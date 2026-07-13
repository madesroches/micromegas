# Release Plan Template for Micromegas

This template is updated after each release with lessons learned.
Last updated: v0.27.0 (2026-07-13)

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
- [ ] Update `README.md` roadmap for vX.Y.0

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
- [ ] Push release branch and all tags: `git push origin release && git push origin vX.Y.0 grafana-vX.Y.0 capi-vX.Y.0 blender-vX.Y.0`

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
```

### Phase 3.5: Docker Images

> **Run this before Phase 4** — `build_docker_images.py` reads the workspace version from `rust/Cargo.toml`. Running it after the Phase 4 bump would tag images with the next dev version.

One-time setup (if not already done on this machine):

```bash
docker buildx create --use
docker run --privileged --rm tonistiigi/binfmt --install arm64
docker login
```

Publish all 6 services for both architectures:

```bash
python3 build/build_docker_images.py \
  ingestion flight-sql admin http-gateway analytics-web monolith \
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
