# Publish Docker Images on Release + Release-Process Sync Plan

Issue: [#1165](https://github.com/madesroches/micromegas/issues/1165)

## Overview

Two coupled goals:

1. **(Issue #1165)** Build and publish Docker images (amd64 + arm64) for the
   Micromegas services as part of a release, so users can `docker pull` an
   official image instead of building from source. Images are tagged with the
   release version and `latest` and pushed to **Docker Hub**
   (`marcantoinedesroches/micromegas-<service>`).

2. **(Surfaced while scoping #1165)** Bring the release process back in sync with
   everything the repo now ships. The newest binary artifact is the **C API**
   (`micromegas-capi`, prebuilt Linux/Windows libs), alongside the **Blender
   extension** that bundles it. Both are already built by tag-triggered
   workflows, but the runbook (`tasks/release_plan_template.md`) never tells the
   maintainer to push those tags, so they don't get released; the post-release
   version bump also misses the Blender manifest. The Docker images (incl. the
   `monolith` binary) must likewise be folded in.

Everything the maintainer does by hand (crates, Python, the GitHub release)
already runs **locally**; the Docker publish follows the same model (local
`docker push` to Docker Hub, where the maintainer is already logged in). The C
API and Blender artifacts are already automated via tag-triggered GitHub Actions
— the only gap there is the runbook not telling the maintainer to push those
tags.

## Current State

### What ships today, and how it's released

| Artifact | Built by | Released by | In runbook? |
|---|---|---|---|
| 15 Rust crates → crates.io | `cargo release` | `build/release.py` (local) | ✅ Phase 1 |
| Python library → PyPI | `poetry build` | `poetry publish` (local) | ✅ Phase 2 |
| Grafana plugin | `grafana/build-plugin.sh` (local) | archive attached to `gh release create vX.Y.0` (the `grafana-vX.Y.0` tag triggers no workflow) | ✅ Phase 3 |
| GitHub release (main) | — | `gh release create vX.Y.0` (local) | ✅ Phase 3 |
| **C API** libs (Linux/Windows) | `build/package_capi.py` | `capi-vX.Y.0` tag → `capi-release.yml` | ❌ **missing** |
| **Blender extension** zip (version from workspace `Cargo.toml`, stamped into the manifest at build time) | `build/build_blender_plugin.py` | `blender-vX.Y.0` tag → `blender-extension.yml` | ❌ **missing** |
| **Docker images** | `build/build_docker_images.py` | nothing yet | ❌ **this issue** |

So a maintainer following the runbook to the letter today would **not** release
the C API or the Blender add-on (the workflows exist but only fire on
`capi-v*` / `blender-v*` tags that the runbook never tells them to push), and
would have **no** Docker publish step at all.

### Docker specifics

Dockerfiles in `docker/` are multi-stage and arch-aware: the builder stage runs
on `--platform=$BUILDPLATFORM` and cross-compiles to `$TARGETARCH` (amd64
native; arm64 via `g++-aarch64-linux-gnu` + the aarch64 rust target), so the
heavy Rust compile doesn't pay QEMU cost. Only the small runtime stage
(`debian:bookworm-slim` + `apt-get ca-certificates`) runs on `$TARGETPLATFORM`,
so arm64 needs QEMU only for that cheap step.

Publishable services (`build/build_docker_images.py` `SERVICES`):

| Service | Dockerfile | Binary |
|---|---|---|
| `ingestion` | `ingestion.Dockerfile` | `telemetry-ingestion-srv` |
| `flight-sql` | `flight-sql.Dockerfile` | `flight-sql-srv` |
| `admin` | `admin.Dockerfile` | `telemetry-admin` |
| `http-gateway` | `http-gateway.Dockerfile` | `http-gateway-srv` |
| `analytics-web` | `analytics-web.Dockerfile` | `analytics-web-srv` |
| `monolith` | `monolith.Dockerfile` | `micromegas-monolith` (all roles in one process) |
| `all` | `all-in-one.Dockerfile` | dev/test only — **not** published |

This covers every server binary, including `monolith`. The other non-library
binaries — `micromegas-capi` (released separately as prebuilt libs, see below),
`micromegas-uri-handler` (a Windows desktop URI/CLI client) and
`telemetry-generator` (a test helper) — are not services and get no Docker
image, which is correct.

`build/build_docker_images.py`:
- Image naming `{DOCKERHUB_USER}/{DOCKERHUB_REPO}-{service}` →
  `marcantoinedesroches/micromegas-<service>`.
- Version from `[workspace.package].version` in `rust/Cargo.toml` (`get_version()`).
- `--push` only on the native amd64 `docker build` path; `--arm64` does buildx
  `--load` and refuses `--push`. **No multi-arch push path exists.**

`docker/docker-compose.monolith.yaml` and the `docker run` examples in
`docker/README.md` already reference `marcantoinedesroches/micromegas-*` —
Docker Hub — so the registry choice below needs no migration. (The README's
Images table still lists bare names without the `marcantoinedesroches/` prefix;
that table is updated as part of the docker/README.md work below.)

### Crate-coverage audit (release.py)

All 15 published library crates have `micromegas-*-v0.26.0` tags and matching
`cargo release -p` lines in `build/release.py` — none are missing. The runbook
already carries a standing "fix release.py if new crates were added" check
(Phase 0). The binaries are **not** crates.io publishes and each has its own
release channel: the `-srv` services and `monolith` ship as Docker images
(above); the newest one, **`micromegas-capi`**, ships as prebuilt Linux/Windows
libs via `capi-release.yml` (and is re-bundled into the Blender extension);
`uri-handler` / `telemetry-generator` are not released.

### Version-bump gap

Post-release bump (runbook Phase 4) lists `rust/Cargo.toml`,
`rust/datafusion-wasm/Cargo.toml`, `python/.../pyproject.toml`,
`grafana/package.json`, `analytics-web-app/package.json`. `capi` and
`monolith` use `version.workspace = true`, so they bump automatically. The
Blender manifest needs **no** bump: although
`blender/micromegas_blender/blender_manifest.toml` carries a hardcoded `version`
(currently `0.27.0`), `sync_manifest_version()` unconditionally overwrites it
with the workspace version on every zip build, so the hardcoded value is
transient and not the source of truth. The only real requirement is that the
workspace is at `X.Y.0` when `blender-vX.Y.0` is tagged — which the runbook
already guarantees, since that tag is pushed in Pre-Release § 5 on the release
commit (workspace at `X.Y.0`), before the Phase 4 post-release bump.

## Design

### Docker: Docker Hub, both arches, version-with-platform tags

**Registry — Docker Hub.** `marcantoinedesroches/micromegas-<service>` — already
the script default, already what the compose file / README reference, and where
the maintainer is already `docker login`'d. Nothing to migrate, no GHCR.

**Both arches.** Publish `linux/amd64` and `linux/arm64`.

**Tagging follows the crate version, with the platform** — i.e. keep the script's
existing per-platform tag scheme (no `v` prefix; version from `get_version()`,
overridable with `--version`). For release `0.26.0`:

| Arch | Tags |
|---|---|
| amd64 | `…/micromegas-<svc>:0.26.0`, `…:latest` |
| arm64 | `…/micromegas-<svc>:0.26.0-arm64`, `…:latest-arm64` |

These are the tags `build_image()` *already computes* in its `arm64` branch
(`f"{version}-arm64"`, `"latest-arm64"`). So no manifest/tagging redesign is
needed — and deliberately **not** a single fused multi-arch manifest, since the
chosen scheme encodes the platform in the tag.

**Minimal change to `build/build_docker_images.py`** (open/closed; the
service→Dockerfile map and tag computation are untouched): the script can already
build both arches and push amd64, but the arm64 branch currently builds with
`docker buildx build --platform linux/arm64 --load` and is gated by an explicit
guard against `--push`. The arm64 branch must switch from `--load` to `--push`
(build-and-push in one buildx invocation) when `--push` is given, and the guard
that rejects `--arm64 --push` is removed:

```python
if args.arm64 and args.push:
    print("error: --push is not supported with --arm64; ...")
    return 1
```

Replace that block with an arm64 push path: when `--arm64 --push`, build and push
in one step with `docker buildx build --platform linux/arm64 --push -t …:{version}-arm64 -t …:latest-arm64 …`
(buildx pushes directly; no separate `docker push`). The existing
`--arm64 --load` (no push) local path stays for single-arch local testing, and
the amd64 `--push` path is unchanged.

Release-time usage (local) — two invocations, one per arch:

```bash
# one-time on the maintainer's machine:
docker buildx create --use
docker run --privileged --rm tonistiigi/binfmt --install arm64   # qemu for arm64 runtime stage
docker login                                                     # Docker Hub

SVCS="ingestion flight-sql admin http-gateway analytics-web monolith"
# amd64 → :0.26.0 / :latest
python3 build/build_docker_images.py $SVCS --push --version 0.26.0
# arm64 → :0.26.0-arm64 / :latest-arm64
python3 build/build_docker_images.py $SVCS --arm64 --push --version 0.26.0
```

### Release-process sync

The fix is mostly **documentation in the runbook**, plus one version-bump line —
the C API and Blender automation already exist.

- Add the missing release tags to runbook Pre-Release Checklist § 5 (Git
  Preparation) and a note
  in the Release Process: push `capi-vX.Y.0` and `blender-vX.Y.0` alongside
  `vX.Y.0` / `grafana-vX.Y.0`. State in § 5 that `capi-vX.Y.0` and
  `blender-vX.Y.0` are created on the **same commit as `vX.Y.0`** (the release
  commit, where the workspace version is `X.Y.0`, before the Phase 4 post-release
  bump) — add them to the existing `git tag` / `git push origin` lines. This is
  required because both workflows build from the tagged commit, but they derive
  the artifact version differently: `capi-release.yml` passes the stripped
  `capi-v` ref to `package_capi.py --version`, whereas `blender-extension.yml`
  runs `build_blender_plugin.py --zip-only` with no version argument — the
  Blender artifact's version comes from the workspace `[workspace.package].version`
  in `rust/Cargo.toml`, which `sync_manifest_version()` stamps into the manifest
  at build time (the `blender-v` ref is used only for the GitHub Release title).
  Each triggers its existing workflow which builds the native
  libs and attaches the archives to a GitHub Release.
- Add a new **Docker images** phase running the local `build_docker_images.py`
  command above, with a verification step
  (`docker buildx imagetools inspect marcantoinedesroches/micromegas-monolith:X.Y.0`
  shows both platforms). Insert it **before** the Phase 4 post-release bump
  (e.g. as "Phase 3.5", immediately after Phase 3), so it runs while the
  workspace is still at `X.Y.0` — `build_docker_images.py` `get_version()` reads
  `[workspace.package].version`, so running it after the Phase 4 bump would tag
  images with the next dev version.
- No Phase 4 bump line for the Blender manifest: its `version` is overwritten
  from the workspace at build time, so a manual bump has no effect on the
  released artifact. The runbook already ensures the workspace is at `X.Y.0` when
  `blender-vX.Y.0` is tagged (§ 5, before the Phase 4 bump), which is the only
  requirement.
- Keep the standing Phase 0 crate-audit check (already present) and extend its
  spirit to "also check for new server binaries → add to
  `build_docker_images.py` `SERVICES` and a Dockerfile."

```
git tag vX.Y.0 grafana-vX.Y.0 capi-vX.Y.0 blender-vX.Y.0   (push all)
        │                │            │             │
   gh release      (no tag CI)   capi CI       blender CI
   (local)         grafana asset + assets      + assets
                   built locally,
                   attached to release
        +
   release.py (crates) · poetry (PyPI) · build_docker_images.py --push (Docker Hub)   ← local
```

## Implementation Steps

1. **Edit `build/build_docker_images.py`** — replace the `--arm64 + --push`
   rejection with an arm64 buildx `--push` path (tags `…:{version}-arm64` /
   `…:latest-arm64`, already computed). Preserve the existing amd64 `--push` and
   `--arm64 --load` (local, no push) behaviour. Also update the now-stale help
   text: the module docstring ("no push") and the `--arm64` argparse help
   ("uses docker buildx, --load only") must be reworded to state that
   `--arm64 --push` is now supported.
2. **Update `tasks/release_plan_template.md`**:
   - New "Phase: Docker Images" with the local both-arch publish (two
     invocations) + inspect verification. Insert it **before** the Phase 4
     post-release bump (e.g. as "Phase 3.5", immediately after Phase 3) so it
     runs while the workspace is at `X.Y.0`; `build_docker_images.py`
     `get_version()` reads `[workspace.package].version`, so placing it after the
     bump would tag images with the next dev version.
   - Pre-Release Checklist § 5 (Git Prep) + Release Process: push `capi-vX.Y.0` and
     `blender-vX.Y.0`; note each triggers its workflow.
   - Fix the stale Phase 3 line (`release_plan_template.md:130`) that claims the
     `grafana-vX.Y.0` tag push triggers GitHub Actions — `grafana-plugin.yml`
     fires only on branch pushes/PRs, not `grafana-v*` tags. Reword so it states
     the Grafana plugin archive is built/attached locally and the `grafana-v*`
     tag fires no workflow (matching the Current State table).
   - Phase 4 bump list: no Blender manifest entry — its `version` is stamped from
     the workspace at build time; ensuring the workspace is at `X.Y.0` at
     blender-tag-push time (already done in § 5) is the only requirement.
   - Phase 0: add "new server binary → add to `SERVICES` + Dockerfile" alongside
     the existing new-crate check.
3. **Update `docker/README.md`** — document the published images, the two
   per-arch publish commands, the one-time buildx/qemu/login setup, and the
   tag scheme (`X.Y.Z` / `latest` for amd64, `X.Y.Z-arm64` / `latest-arm64` for
   arm64). Also prefix the README's Images table entries for the 6 published
   services (`ingestion`, `flight-sql`, `admin`, `http-gateway`, `analytics-web`,
   `monolith`) with `marcantoinedesroches/` and note the published tag scheme
   (the `docker run` examples and compose file already point to Docker Hub).
   Leave `all`/`micromegas-all` (all-in-one, dev/test) and
   `micromegas-github-runner` (self-hosted CI runner) unprefixed — they are not
   published to Docker Hub.
   - Remove or correct the `python build/build_docker_images.py --push all`
     example in the README "Building" section — it instructs publishing the
     dev/test-only all-in-one image, which contradicts the decision that `all`
     is never published.

No change is required to `release.py` (crate coverage is complete) or to the
Dockerfiles/compose (monolith already covered, registry already Docker Hub).

## Files to Modify

- `build/build_docker_images.py` — replace the `--arm64 + --push` guard with an
  arm64 buildx `--push` path.
- `tasks/release_plan_template.md` — Docker phase (both arches), capi/blender tag
  steps, new-binary check.
- `docker/README.md` — published-image docs, both-arch publish commands, tag
  scheme (incl. the `-arm64` suffix).

## Trade-offs

- **Local publish vs GitHub Actions (Docker)**: local chosen — consistent with
  the crate/Python/GitHub publish steps, no CI secrets, no runner build cost,
  uses the maintainer's existing Docker Hub login. Cost: manual, depends on the
  maintainer's machine + a one-time buildx/qemu setup. A tag-triggered workflow
  stays a clean future option (it would call the same extended script), but is
  out of scope.
- **Docker Hub vs GHCR**: Docker Hub chosen (already the default, already
  referenced, already the local login target → zero migration).
- **Per-platform tags vs fused multi-arch manifest**: per-platform tags
  (`:0.26.0`, `:0.26.0-arm64`) chosen — it matches the script's existing scheme
  and the decision to encode the platform in the tag, and needs only the guard
  removed. A fused manifest (one tag, both arches) would be more
  `docker pull`-friendly but is a larger change and discards the platform-in-tag
  convention.
- **Extend the existing script vs new script**: extending keeps one source of
  truth for the service list and tagging and reuses the maintainer's `--push`
  habit.
- **Fix the runbook vs build new automation for capi/blender**: those workflows
  already work; the only real defect is the runbook not invoking them. Documenting
  the tag pushes is the minimal correct fix (DRY — don't duplicate the existing
  CI).
- **Publish `all-in-one`?** No — dev/test only; `monolith` is the supported
  single-image path.

## Performance

A release builds 6 images × 2 arches with full release-mode Rust compiles. Cross-
compilation avoids QEMU for the compile itself (only the cheap runtime `apt-get`
is emulated). Locally this reuses the maintainer's warm cargo/docker layer cache
across releases. arm64 can be deferred (amd64-only first) if build time is a
concern — see open questions.

## Documentation

- `docker/README.md` — published images, publish command, one-time setup, tag
  scheme.
- `tasks/release_plan_template.md` — the authoritative runbook updates above.
- Optional follow-up: a short "Run with Docker" pointer in the top-level
  `README.md` / mkdocs install docs.

## Testing Strategy

- **Script behaviour**: a plain build, `--push` (amd64), and `--arm64 --load`
  (local, no push) must all still work (existing behaviour intact); `--arm64
  --push` now builds and pushes the `-arm64`-suffixed tags instead of erroring.
- **Both-arch publish dry run**: push `monolith` for both arches with a throwaway
  version, then confirm `…/micromegas-monolith:<v>` (amd64) and
  `…/micromegas-monolith:<v>-arm64` both exist and report the expected
  architecture (`docker buildx imagetools inspect` / `docker image inspect`).
- **Smoke test**: `docker compose -f docker/docker-compose.monolith.yaml up`
  against the pushed image; hit `http://localhost:3000`.
- **Runbook dry-run review**: walk the updated template and confirm every row in
  the "what ships today" table maps to a concrete step (crates, Python, Grafana,
  GitHub release, C API tag, Blender tag, Docker push).
- **End-to-end**: next real release — all 6 images present with `:<version>` +
  `:latest`; `capi-v*` and `blender-v*` GitHub Releases produced.

## Decisions (resolved)

1. **Registry**: Docker Hub only (no GHCR).
2. **Arches**: both `linux/amd64` and `linux/arm64`.
3. **Tags**: follow the crate version with a platform suffix — amd64 `:X.Y.Z` /
   `:latest`, arm64 `:X.Y.Z-arm64` / `:latest-arm64`. No `v` prefix; no fused
   multi-arch manifest.
4. **capi + blender cadence**: always released together with the main version
   (same `X.Y.0`), exactly like the Grafana plugin and Python library — so every
   release pushes `vX.Y.0`, `grafana-vX.Y.0`, `capi-vX.Y.0`, and
   `blender-vX.Y.0` together.

## Open Questions

None outstanding.
