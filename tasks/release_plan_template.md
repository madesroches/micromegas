# Release Plan Template for Micromegas

This template is updated after each release with lessons learned.
Last updated: v0.22.0 (2026-03-13)

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

### 0. Fix release.py (if new crates were added)

- [ ] Verify any new published crates are in `build/release.py` in the correct dependency order
- [ ] Verify new crates in the wasm workspace have explicit `version = "^X.Y"` on all micromegas path deps

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

### 3. Documentation Updates

- [ ] Review git log: `git log --oneline vX.Y.0..HEAD`
- [ ] Update `CHANGELOG.md` — move Unreleased entries to `## vX.Y.0 - <date>` section
- [ ] Update `grafana/CHANGELOG.md` with version sync entry
- [ ] Update `README.md` roadmap for vX.Y.0

### 4. Grafana Plugin Preparation

- [ ] Build plugin archive: `./build-plugin.sh` (from `grafana/` directory)

### 5. Git Preparation

- [ ] Commit changelog and doc updates
- [ ] Create release tag: `git tag vX.Y.0`
- [ ] Create grafana tag: `git tag grafana-vX.Y.0`
- [ ] Push release branch and tags: `git push origin release && git push origin vX.Y.0 grafana-vX.Y.0`

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

The `grafana-vX.Y.0` tag push triggers GitHub Actions. Also create the main GitHub release:

```bash
gh release create vX.Y.0 \
  --title "Micromegas vX.Y.0 - <tagline>" \
  --notes "..." \
  grafana/micromegas-micromegas-datasource.zip
```

### Phase 4: Post-Release Version Bump to X.Z.0

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
