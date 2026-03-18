# Container-Based Dev Worker for CI Builds

## Overview

Self-hosted GitHub Actions runner infrastructure that lets developer workstations contribute to CI builds using Docker container isolation. Builds from trusted authors route to available dev workers, falling back to GitHub-hosted runners when no worker is online.

## Design

### Architecture

```
┌─────────────────────────────────────────────────────┐
│  GitHub Actions                                      │
│                                                      │
│  ┌──────────────┐    online + trusted?  ┌──────────┐│
│  │ check-runner │───── yes ────────────▶│ dev-worker││
│  │ (ubuntu-     │                       │ (self-   ││
│  │  latest)     │───── no ────────────▶ │  hosted) ││
│  └──────────────┘      │                └──────────┘│
│                        ▼                             │
│                  ┌──────────┐                        │
│                  │ ubuntu-  │                        │
│                  │ latest   │                        │
│                  └──────────┘                        │
└─────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────┐
│  Developer Workstation                                │
│                                                       │
│  ┌──────────────────────────────────────────────┐     │
│  │  Docker                                      │     │
│  │  ┌────────────────────────────────────┐      │     │
│  │  │  micromegas-runner (persistent)    │      │     │
│  │  │  - GitHub Actions runner agent     │      │     │
│  │  │  - Rust + clang + mold             │      │     │
│  │  │  - Node 20 + Yarn                 │      │     │
│  │  │  - Go 1.21 + Mage                 │      │     │
│  │  │  - Python 3 + Poetry               │      │     │
│  │  │  - wasm-pack + wasm-bindgen-cli    │      │     │
│  │  │  - Firefox + geckodriver           │      │     │
│  │  │                                    │      │     │
│  │  │  Build cache on container FS       │      │     │
│  │  │  (warm while container is up)      │      │     │
│  │  └────────────────────────────────────┘      │     │
│  └──────────────────────────────────────────────┘     │
└──────────────────────────────────────────────────────┘
```

### Key design decisions

- **Persistent container** — the runner stays online and handles multiple jobs back-to-back. No re-registration between jobs, no session conflicts.
- **Container filesystem as cache** — build artifacts live on the container's filesystem at `/cache`. The cache stays warm as long as the container is running. Stopping the container (`--rm`) clears the cache; `--rotate-cache` / `--rotate-at` restart with a warming build.
- **Fixed container name** (`micromegas-runner`) — Docker refuses to start a second container with the same name, preventing concurrent instances from corrupting the shared cache.
- **Unique runner registration name** (`micromegas-runner-<uuid>`) — each `docker run` registers a fresh runner with GitHub, avoiding stale session conflicts from previous registrations.
- **Trusted authors only** — `check-runner` gates routing via a `case` statement matching `madesroches` and `madesroches-ubi`. Fork PRs always use GitHub-hosted runners.
- **`timeout-minutes: 30`** on all dev-worker jobs — prevents indefinite queueing if the runner disappears.
- **CODEOWNERS** protects `.github/`, `docker/github-runner.Dockerfile`, and `build/dev_worker.py`.

### Security

1. Only trusted authors' builds route to the dev-worker (PR author check, not actor)
2. Container isolation — bridge networking, no `--privileged`, no Docker-in-Docker
3. Registration token passed via bind-mounted file, never via env var or CLI
4. PAT stored in file (`chmod 600`) or env var, never on command line
5. Graceful degradation — offline worker = GitHub-hosted runners, zero delay

### Workflow pattern

Each workflow calls the reusable `check-runner.yml`:

```yaml
jobs:
  check-runner:
    uses: ./.github/workflows/check-runner.yml
    secrets: inherit

  build:
    needs: check-runner
    runs-on: ${{ needs.check-runner.outputs.runner }}
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
      - name: Setup tools
        if: needs.check-runner.outputs.runner == 'ubuntu-latest'
        # ... tool installation skipped on dev-worker (pre-installed)
      - name: Run CI
        run: |
          if [ -z "$CARGO_BUILD_JOBS" ]; then unset CARGO_BUILD_JOBS; fi
          ./build/rust_ci.py native
        env:
          # Empty on dev-worker → unset → cargo uses all CPUs
          CARGO_BUILD_JOBS: ${{ needs.check-runner.outputs.runner == 'ubuntu-latest' && '2' || '' }}
```

Skip workflows (`build-skip.yml`, `web-build-skip.yml`) include a stub `check-runner` job for required status check matching.

### Nightly cache rotation

`--rotate-cache` or `--rotate-at HOUR` stops the container (clearing the cache since `--rm` removes the filesystem), starts a fresh one, and triggers a warming build on main via `workflow_dispatch`.

## Files

**New:**
- `docker/github-runner.Dockerfile` — runner container image
- `docker/github-runner-entrypoint.sh` — container entrypoint
- `build/dev_worker.py` — management script
- `.github/workflows/check-runner.yml` — reusable runner routing workflow
- `.github/CODEOWNERS` — protects CI infrastructure files

**Modified:**
- `.github/workflows/rust.yml` — check-runner, conditional steps, `workflow_dispatch`, dynamic `CARGO_BUILD_JOBS`
- `.github/workflows/grafana-plugin.yml` — check-runner, conditional steps, E2E skipped on dev-worker
- `.github/workflows/analytics-web-app.yml` — check-runner, conditional steps
- `.github/workflows/build-skip.yml` — check-runner stub
- `.github/workflows/web-build-skip.yml` — check-runner stub
- `docker/README.md` — added runner image to table
- `mkdocs/docs/development/build.md` — setup instructions

## Usage

```bash
# One-time PAT setup
echo "ghp_xxx" > ~/.config/micromegas/runner-pat && chmod 600 ~/.config/micromegas/runner-pat
gh secret set RUNNER_PAT  # same PAT as repo secret

# Start the worker
python3 build/dev_worker.py

# With resource limits
python3 build/dev_worker.py --cpus 8 --memory 16g

# With nightly cache rotation at 03:00
python3 build/dev_worker.py --rotate-at 3

# Build image only
python3 build/dev_worker.py --build-image

# Clear cache and exit
python3 build/dev_worker.py --clear-cache

# Rotate cache now, then start worker
python3 build/dev_worker.py --rotate-cache
```

## Trade-offs

| Decision | Chosen | Rejected |
|----------|--------|----------|
| Runner mode | Persistent container — handles jobs back-to-back | Ephemeral per-job — clean but fragile re-registration gaps |
| Cache strategy | Container filesystem — simple, cache warm while container is up | Docker volume — survives restarts but adds management complexity |
| Runner routing | API check from ubuntu-latest (~30s overhead) | Timeout-based fallback — `timeout-minutes` doesn't apply to queue wait |
| Trust model | Trusted authors only (madesroches, madesroches-ubi) | All PRs — unacceptable security risk |
| Orchestration | Python script + Docker | ARC/Kubernetes — overkill for one machine |
