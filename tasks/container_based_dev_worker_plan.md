# Container-Based Dev Worker for CI Builds

## Overview

Add self-hosted GitHub Actions runner infrastructure that lets developer workstations contribute to CI builds safely using Docker container isolation. Builds from the repo owner preferentially route to available dev workers, falling back gracefully to GitHub-hosted runners when no worker is online. The design must be safe enough for a corporate environment: untrusted code never runs on the worker, and every job executes inside an ephemeral container.

## Current State

All CI runs on `ubuntu-latest` GitHub-hosted runners:
- **Rust** (`.github/workflows/rust.yml`): Two jobs — `native` and `wasm`, each on 2-core runners with aggressive disk cleanup, `mold` linker, `CARGO_BUILD_JOBS=2` to avoid OOM
- **Grafana** (`.github/workflows/grafana-plugin.yml`): Node 20 + Go 1.21 + Mage + Playwright E2E
- **Analytics Web** (`.github/workflows/analytics-web-app.yml`): Node (from .nvmrc) + Yarn
- **Docs** (`.github/workflows/publish-docs.yml`): cargo doc + MkDocs + presentations

Build scripts are Python-based (`build/rust_ci.py`, `build/grafana_ci.py`, `build/analytics_web_ci.py`).

Pain points: Rust builds are memory-constrained (limited to 2 parallel jobs), require ~20GB disk cleanup, and take ~10 minutes. A workstation with more RAM and CPU would significantly speed these up.

Prior research in `tasks/github-actions-runner-alternatives.md` evaluated cloud alternatives (RunsOn, EC2 Spot, Hetzner, etc.). This plan takes a different approach: using existing developer hardware at zero marginal cost.

## Threat Model & Security Design

### Corporate safety requirements

1. **No untrusted code execution on the workstation** — only the repo owner's PRs and pushes to main run on the worker
2. **Container isolation** — every job runs in a fresh ephemeral Docker container; nothing persists to the host beyond cached build artifacts in a Docker volume
3. **No host network exposure** — containers use bridge networking, not host mode
4. **No privileged containers** — no `--privileged`, no Docker-in-Docker
5. **Graceful degradation** — if the worker is offline, CI continues on GitHub-hosted runners with zero delay

### Trust boundary

The workflow already has `auto-approve-owner.yml` which checks `github.event.pull_request.user.login == 'madesroches'`. The self-hosted runner jobs will use the same gate — the `check-runner` job runs on `ubuntu-latest` and only routes to the self-hosted runner when:
1. The **PR author** (for `pull_request` events) or **pusher** (for `push` events) is the repo owner — importantly, for PRs this checks `github.event.pull_request.user.login` (the actual code author), not `github.actor` which reflects who triggered the run (e.g., a maintainer re-running someone else's PR). For push events, `github.event.pusher.name` identifies who pushed the commits.
2. The runner is reported as online via the GitHub API

Fork PRs from external contributors always run on GitHub-hosted runners. This is the key security property: the self-hosted runner never executes untrusted code.

**Same-repo collaborator PR caveat:** For `pull_request` events triggered by same-repo branches (not forks), GitHub Actions runs the workflow from the merge of base and head. This means a collaborator with push access could submit a PR that modifies the workflow file to remove the owner check, causing their code to route to the dev-worker. Fork PRs are not affected — GitHub always uses the base branch workflow for those. Mitigations:
1. This repo is currently single-owner, so no collaborators can exploit this
2. A CODEOWNERS file (`.github/CODEOWNERS`) requires owner approval for changes to `.github/`, `docker/github-runner.Dockerfile`, and `build/dev_worker.py` — enforced as part of this plan (see Phase 1)
3. For stronger isolation: move the `check-runner` logic to a reusable workflow in a separate private repo (called workflows cannot be overridden by the calling PR)

## Design

### Architecture

```
┌─────────────────────────────────────────────────────┐
│  GitHub Actions                                      │
│                                                      │
│  ┌──────────────┐    online + owner?    ┌──────────┐│
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

┌──────────────────────────────────────────────────────────┐
│  Developer Workstation                                    │
│                                                           │
│  ┌──────────────────────────────────────────────────┐     │
│  │  Docker                                          │     │
│  │  ┌────────────────────────────────────────┐      │     │
│  │  │  Runner Container (ephemeral)          │      │     │
│  │  │  - GitHub Actions runner agent         │      │     │
│  │  │  - Rust toolchain + mold               │      │     │
│  │  │  - Node 20 + Yarn                     │      │     │
│  │  │  - Go 1.21                             │      │     │
│  │  │  - Python 3 + Poetry                   │      │     │
│  │  │  - wasm-pack + wasm-bindgen-cli        │      │     │
│  │  │                                        │      │     │
│  │  │  CARGO_HOME=/cache/cargo-home          │      │     │
│  │  │  CARGO_TARGET_DIR=/cache/target-native │      │     │
│  │  │         (or /cache/target-wasm)        │      │     │
│  │  └──────────┬─────────────────────────────┘      │     │
│  │             │                                    │     │
│  │  ┌──────────▼─────────────────────────────┐      │     │
│  │  │  micromegas-build-cache (Docker volume) │      │     │
│  │  │                                         │      │     │
│  │  │  /cache/                                │      │     │
│  │  │  ├── cargo-home/                        │      │     │
│  │  │  │   ├── registry/  (crate downloads)   │      │     │
│  │  │  │   └── git/       (git deps)          │      │     │
│  │  │  ├── target-native/ (compiled artifacts)│      │     │
│  │  │  └── target-wasm/   (WASM artifacts)    │      │     │
│  │  └────────────────────────────────────────┘      │     │
│  └──────────────────────────────────────────────────┘     │
└──────────────────────────────────────────────────────────┘
```

### Component 1: Runner Container Image

A Dockerfile (`docker/github-runner.Dockerfile`) that packages:
- GitHub Actions runner agent (latest)
- Rust stable toolchain
- `mold` linker
- `cargo-machete`
- `wasm-pack`, `wasm-bindgen-cli` (version extracted from Cargo.lock at build time)
- `wasm32-unknown-unknown` target
- Node 20 + Yarn
- Go 1.21 + Mage (`go install github.com/magefile/mage@latest`)
- Playwright + Chromium (for Grafana E2E tests)
- Python 3 + pip
- Poetry

The container runs the GitHub Actions runner in **ephemeral mode** (`--ephemeral`): it picks up one job, runs it, then exits. A management script restarts it for the next job, ensuring a clean environment every time.

**Entrypoint script** (`docker/github-runner-entrypoint.sh`):
```bash
#!/usr/bin/env bash
set -euo pipefail

# 1. Read the registration token from the bind-mounted secret file.
TOKEN=$(cat /run/secrets/registration-token)

# 2. Configure the runner (ephemeral, labeled, non-interactive).
./config.sh \
  --url "https://github.com/${REPO}" \
  --token "${TOKEN}" \
  --name "${RUNNER_NAME}" \
  --labels "dev-worker,linux,${ARCH}" \
  --work _work \
  --ephemeral \
  --unattended \
  --replace

# 3. Remove the token inside the container — it is single-use and already consumed.
rm -f /run/secrets/registration-token

# 4. Run the runner agent. It picks up one job, executes it, then exits.
./run.sh
```

Environment variables `REPO`, `RUNNER_NAME`, and `ARCH` are set by the management script via `docker run -e`.

### Component 2: Runner Management Script

A Python script (`build/dev_worker.py`) that:
1. Builds (or pulls) the runner container image
2. Obtains a registration token from the GitHub API
3. Starts the runner container with:
   - Ephemeral mode (`--ephemeral`)
   - Labels: `dev-worker`, `linux`, `x64` (or `arm64`)
   - A single named Docker volume `micromegas-build-cache` mounted at `/cache`
   - Registration token passed via a bind-mounted temporary file at `/run/secrets/registration-token` (never passed via env var)
   - Bridge networking (no host mode)
   - No resource limits by default (use full workstation); optional `--cpus` / `--memory` flags if needed
4. When the container exits (job done or idle timeout), restarts a fresh container
5. Handles SIGINT/SIGTERM for clean shutdown and runner deregistration

Usage:
```bash
# Configure the PAT (one-time setup, choose one):
#   Option 1 — environment variable:
export MICROMEGAS_RUNNER_PAT=ghp_xxx
#   Option 2 — file (chmod 600 is enforced by the script):
echo "ghp_xxx" > ~/.config/micromegas/runner-pat
chmod 600 ~/.config/micromegas/runner-pat

# SECURITY: Never pass the PAT on the command line — it would be visible
# in shell history, `ps aux`, and system audit logs.

# Start the worker (runs until stopped)
python3 build/dev_worker.py

# With resource limits
python3 build/dev_worker.py --cpus 8 --memory 16g

# Clear the entire build cache (stops runner, deletes volume, exits)
python3 build/dev_worker.py --clear-cache
# (equivalent to: docker volume rm micromegas-build-cache)

# Rotate cache nightly: clear cache, restart worker, trigger warming build
python3 build/dev_worker.py --rotate-cache

# Stop gracefully
Ctrl+C  (or kill with SIGTERM)
```

### Component 3: Workflow Changes

Each workflow gets a `check-runner` job that gates routing. The pattern:

```yaml
jobs:
  check-runner:
    runs-on: ubuntu-latest
    outputs:
      runner: ${{ steps.check.outputs.runner }}
    steps:
      - id: check
        run: |
          # Default to GitHub-hosted runner (fail-safe: if anything below errors,
          # the job runs on ubuntu-latest rather than failing or routing to dev-worker).
          echo "runner=ubuntu-latest" >> $GITHUB_OUTPUT

          # Only route to dev-worker for repo owner.
          # For pull_request events, BUILD_AUTHOR is the PR author (not the actor
          # who triggered the run — a maintainer re-running someone else's PR would
          # set github.actor to themselves, bypassing the gate).
          # For push events, BUILD_AUTHOR is the pusher (github.event.pusher.name).
          if [ "$BUILD_AUTHOR" != "madesroches" ]; then
            echo "Not repo owner ($BUILD_AUTHOR), using GitHub-hosted runner"
            exit 0
          fi
          # Check if any dev-worker is online.
          # NOTE: The runners API (GET /repos/{owner}/{repo}/actions/runners) requires
          # admin-level access. The default GITHUB_TOKEN does not have this scope —
          # its permission model only covers actions, contents, pull-requests, etc.
          # A dedicated PAT (secrets.RUNNER_PAT) with fine-grained `manage_runners:self-hosted`
          # scope is required. This is the same PAT used by the management script
          # (Component 5), stored as a repository secret.
          ONLINE=$(gh api "repos/${REPO}/actions/runners" \
            --jq '[.runners[] | select(.labels[].name == "dev-worker") | select(.status == "online")] | length')
          if [ "$ONLINE" -gt "0" ]; then
            echo "runner=dev-worker" >> $GITHUB_OUTPUT
            echo "Dev worker online, routing build there"
          else
            echo "No dev worker online, using GitHub-hosted runner"
          fi
        env:
          GH_TOKEN: ${{ secrets.RUNNER_PAT }}
          REPO: ${{ github.repository }}
          # For pull_request events: use the PR author login (immutable, set when
          # the PR is created — cannot be changed by re-running the workflow).
          # For push events: use the pusher name (who actually pushed the commits,
          # not the commit author which can be freely set via git config).
          # Values are passed via env vars to avoid script injection — never
          # interpolate ${{ }} expressions directly inside run: shell scripts.
          BUILD_AUTHOR: ${{ github.event.pull_request.user.login || github.event.pusher.name || github.actor }}

  native:
    needs: check-runner
    runs-on: ${{ needs.check-runner.outputs.runner }}
    steps:
      # Conditional setup: skip disk cleanup and tool installation on dev-worker
      # (tools are pre-installed in the container image)
      - uses: actions/checkout@v4

      - name: Free up disk space
        if: ${{ needs.check-runner.outputs.runner == 'ubuntu-latest' }}
        run: |
          # ... existing disk cleanup ...

      - name: Install mold linker
        if: ${{ needs.check-runner.outputs.runner == 'ubuntu-latest' }}
        run: |
          sudo apt-get update -qq
          sudo apt-get install -y mold

      - name: Install cargo-machete
        if: ${{ needs.check-runner.outputs.runner == 'ubuntu-latest' }}
        run: cargo install cargo-machete

      - name: Run native CI
        run: ./build/rust_ci.py native
        env:
          # Allow more parallelism on dev-worker (more RAM available)
          CARGO_BUILD_JOBS: ${{ needs.check-runner.outputs.runner == 'dev-worker' && '0' || '2' }}
          # On dev-worker, CARGO_TARGET_DIR and CARGO_HOME are already set
          # in the container environment — no workflow config needed.
          # On GitHub-hosted, they are unset, so cargo uses the default rust/target/.
```

For the WASM job on dev-worker, the workflow overrides the target dir:
```yaml
  wasm:
    needs: check-runner
    runs-on: ${{ needs.check-runner.outputs.runner }}
    steps:
      - uses: actions/checkout@v4

      - name: Run WASM CI
        run: |
          # GitHub Actions sets env vars to empty string rather than leaving them
          # unset. On GitHub-hosted runners CARGO_TARGET_DIR must be truly unset
          # because rust/datafusion-wasm/build.py checks
          # `if "CARGO_TARGET_DIR" in os.environ` and Path('') would resolve to
          # the current directory, breaking the build.
          if [ -z "$CARGO_TARGET_DIR" ]; then unset CARGO_TARGET_DIR; fi
          ./build/rust_ci.py wasm
        env:
          # Override to WASM-specific cache dir on dev-worker (native and WASM targets conflict).
          # On GitHub-hosted this evaluates to '' which the shell guard above unsets.
          CARGO_TARGET_DIR: ${{ needs.check-runner.outputs.runner == 'dev-worker' && '/cache/target-wasm' || '' }}
```

Key workflow properties:
- The `check-runner` job adds ~30s overhead (runs on GitHub-hosted)
- On dev-worker: skip disk cleanup, skip tool installation, allow full parallelism (`CARGO_BUILD_JOBS=0` = use all CPUs)
- On dev-worker: `CARGO_HOME` and `CARGO_TARGET_DIR` are set by the container environment (for native builds) or overridden per-step (for WASM builds)
- On GitHub-hosted: these env vars must be **unset** (not empty string), so cargo and `build.py` use their defaults — existing behavior preserved exactly
- `CARGO_BUILD_JOBS` is dynamic: `0` (all cores) on dev-worker, `2` on GitHub-hosted

### Component 4: Build Cache

A single Docker volume (`micromegas-build-cache`) mounted at `/cache` inside the container holds all persistent build state:

```
/cache/
├── cargo-home/          # CARGO_HOME — downloaded crate sources + git deps
│   ├── registry/
│   └── git/
├── target-native/       # CARGO_TARGET_DIR for native builds
└── target-wasm/         # CARGO_TARGET_DIR for WASM builds
```

**How Rust finds the cache:** The container sets these environment variables, which cargo respects natively:
- `CARGO_HOME=/cache/cargo-home` — redirects all crate downloads and git checkouts
- `CARGO_TARGET_DIR=/cache/target-native` — redirects all compiled artifacts for native builds

For WASM builds, the workflow step overrides: `CARGO_TARGET_DIR=/cache/target-wasm`. This keeps native and WASM artifacts separate (they use different targets and would conflict).

Since `CARGO_TARGET_DIR` is an absolute path, it works regardless of the workspace checkout location. Both the main workspace (`rust/`) and any sub-crate builds will write to the same target directory, which is exactly what cargo expects when `CARGO_TARGET_DIR` is set.

**No custom staleness detection needed.** Cargo's built-in incremental compilation already handles dependency and toolchain changes correctly — when `Cargo.lock` changes or the compiler version differs, cargo recompiles affected crates automatically. Adding a custom fingerprint layer would duplicate this logic and introduce complexity (e.g., needing repo access before checkout). If the cache becomes corrupt, two mechanisms handle it:

**Easy cache management:**
```bash
# Clear the entire build cache — one command
python3 build/dev_worker.py --clear-cache
# (equivalent to: docker volume rm micromegas-build-cache)

# Inspect cache size
docker system df -v | grep micromegas-build-cache
```

The `--clear-cache` flag stops the runner if running, removes the Docker volume, and exits. Next start creates a fresh volume automatically. There are no partial-clear options — if something is wrong, nuke and rebuild. Simple.

**Safety properties:**
1. Cargo's native fingerprinting handles dependency and toolchain changes automatically
2. Worst case if the cache is somehow corrupt: `--clear-cache`, next build is clean
3. The nightly cache rotation (Component 6) wipes the entire volume daily, bounding any cache poisoning exposure to ~24 hours

### Component 5: PAT-Based Registration Token Management

The management script uses a **fine-grained GitHub PAT** to automatically obtain runner registration tokens. No manual GitHub UI interaction needed. This same PAT must also be stored as the repository secret `RUNNER_PAT` so the `check-runner` workflow job can query the runners API (the default `GITHUB_TOKEN` lacks the required admin scope — see Component 3).

**PAT requirements:**
- Fine-grained PAT scoped to the single repository (`madesroches/micromegas`)
- Required permission: **`Administration: Read and write`** — this is the minimum fine-grained PAT scope that grants access to `POST /repos/{owner}/{repo}/actions/runners/registration-token` and `GET /repos/{owner}/{repo}/actions/runners`. There is no finer-grained "runners-only" scope available for repository-level fine-grained PATs. Note that `Administration` also grants branch protection and webhook management; mitigate by setting the shortest practical expiry and limiting the PAT to the single repo.
- For organization-owned repos, the narrower **`organization_self_hosted_runners:write`** scope exists, but this repo is user-owned.
- **Classic PAT alternative:** A classic PAT with `repo` scope also works and may be simpler. The `repo` scope is broader (all repo data), but for a single-owner private workflow this is acceptable.
- Expiry: set to the **shortest practical lifetime** (e.g., 90 days). The management script should print a warning when the PAT is within 14 days of expiry.

**PAT storage** (checked in this order):
1. Environment variable: `MICROMEGAS_RUNNER_PAT`
2. File: `~/.config/micromegas/runner-pat` (plain text, `chmod 600`)
3. If neither found, the script exits with a clear error message explaining how to set it up

The PAT is never passed on the command line (avoids shell history exposure) and never mounted into the runner container (the container only receives the short-lived registration token).

**Flow:**
1. Script reads PAT from env var or file
2. Calls GitHub API to generate a short-lived registration token (expires in 1 hour)
3. Writes the registration token to a host-side temporary file (`mktemp`, `chmod 600`) and starts the container with that file bind-mounted read-only at `/run/secrets/registration-token`:
   ```
   --mount type=bind,source=/tmp/xxx,target=/run/secrets/registration-token,readonly
   ```
   The host file is deleted in a `finally` block immediately after `docker run` returns (whether the container succeeds or fails). This is a simple bind mount of a single file — no tmpfs layering. The registration token is single-use (consumed during runner registration) and short-lived (1 hour expiry), so even if the host file exists for the duration of the container run, the token has no value after registration completes.
4. The container entrypoint reads the token from `/run/secrets/registration-token`, registers the runner, then unlinks the file inside the container. The token is never in the container's environment variables and is not visible via `docker inspect` or `/proc/1/environ`.
5. Container runs one job, exits
6. Script obtains a fresh registration token for the next container

**Why not pass the registration token as an environment variable?** Docker environment variables are visible via `docker inspect` and inside the container at `/proc/1/environ`. Any workflow step (including scripts from dependencies) can read all environment variables. Although registration tokens are short-lived (1 hour) and single-use, using a bind-mounted file avoids unnecessary exposure entirely.

### Component 6: Nightly Cache Rotation

The build cache is wiped and rebuilt from scratch every night. This eliminates the risk of persistent cache poisoning (e.g., tampered files in `cargo-home/bin/` or `registry/src/` surviving across builds indefinitely) and ensures developers always hit a warm cache during working hours.

**How it works:**

The management script supports a `--rotate-cache` flag that combines cache wipe, worker restart, and build trigger into a single atomic operation:

1. Stop the current runner container (if running)
2. Delete the Docker volume (`micromegas-build-cache`)
3. Start a fresh runner container (with a new empty volume)
4. Trigger a full CI build on main via `gh workflow run rust.yml --ref main`
5. The cold build routes to the now-online dev-worker (~10 min), populating `cargo-home/`, `target-native/`, and `target-wasm/`
6. By morning, all daytime builds hit warm cache — no developer ever waits on a cold build

The script waits for the new runner to register as online (polling `gh api` for up to 60 seconds) before triggering the workflow dispatch, so the warming build reliably routes to the dev-worker.

**Cron setup (one-time):**
```bash
# Add to the workstation's crontab (crontab -e):
# Nightly at 03:00 — wipe cache, restart worker, trigger warming build
0 3 * * * cd /home/madesroches/git/micromegas && python3 build/dev_worker.py --rotate-cache
```

The `gh workflow run` command (called internally by `--rotate-cache`) requires `workflow_dispatch` to be added as a trigger to `rust.yml` (restricted to the default branch). This is safe — `workflow_dispatch` runs the workflow from the target branch (not from a PR), so it cannot be used to inject untrusted code.

**Workflow change for cache warming:**
```yaml
on:
  push:
    branches: [ "main" ]
    paths: [ ... ]
  pull_request:
    branches: [ "main" ]
    paths: [ ... ]
  workflow_dispatch:  # For nightly cache warming — only available on default branch
```

**Security benefit:** Without nightly rotation, a compromised dependency's `build.rs` could plant persistent malicious artifacts in `cargo-home/` (e.g., a trojan in `cargo-home/bin/`, or tampered extracted sources in `registry/src/`). These artifacts would survive indefinitely since cargo doesn't re-extract crates that are already present. The nightly full wipe bounds this exposure to at most ~24 hours and ensures the cache is always rebuilt from a clean state against the latest main branch.

## Implementation Steps

### Phase 1: Security Baseline & Runner Container Image — DONE
1. ~~Add `.github/CODEOWNERS` requiring owner approval for `.github/`, `docker/github-runner.Dockerfile`, and `build/dev_worker.py`~~ — already existed
2. ~~Create `docker/github-runner.Dockerfile` with all build dependencies~~ — done
   - Also created `docker/github-runner-entrypoint.sh`

### Phase 2: Management Script — DONE
3. ~~Create `build/dev_worker.py`~~ — done
4. ~~Handle registration token acquisition, container lifecycle, and clean shutdown~~ — done
   - PAT from env var (`MICROMEGAS_RUNNER_PAT`) or file (`~/.config/micromegas/runner-pat`)
   - Registration token via GitHub API, passed as bind-mounted secret file
   - Ephemeral container loop with SIGINT/SIGTERM handling
   - `--cpus` / `--memory` resource limits
   - `--build-image` for pre-building the image

### Phase 3: Workflow Changes — DONE
5. ~~Add `check-runner` job to `.github/workflows/rust.yml`~~ — done
6. ~~Make `native` and `wasm` jobs depend on `check-runner` and use dynamic `runs-on`~~ — done
7. ~~Add conditional steps (skip disk cleanup and tool install on dev-worker)~~ — done
8. ~~Adjust `CARGO_BUILD_JOBS` dynamically~~ — done (`0` on dev-worker, `2` on GitHub-hosted)

### Phase 4: Remaining Workflows — DONE
9. ~~Apply the same pattern to `grafana-plugin.yml`~~ — done (E2E tests skipped on dev-worker since they need Docker)
10. ~~Apply the same pattern to `analytics-web-app.yml`~~ — done
11. ~~Add a `check-runner` stub job to `build-skip.yml` and `web-build-skip.yml`~~ — done
12. **Not included:** `publish-docs.yml` — docs builds are lightweight (cargo doc + MkDocs) and don't benefit from the build cache, so they remain on GitHub-hosted runners only.

### Phase 5: Nightly Cache Rotation — DONE
13. ~~Add `workflow_dispatch` trigger to `rust.yml`~~ — done
14. ~~Implement `--rotate-cache` flag in `dev_worker.py`~~ — done (clear cache → restart worker → wait for online → trigger warming build)
15. ~~Set up nightly cache rotation~~ — done (built-in `--rotate-at HOUR` flag, no cron needed)

### Phase 6: Documentation — DONE
16. ~~Add github-runner image to `docker/README.md`~~ — done
17. ~~Add setup instructions to `mkdocs/docs/development/build.md`~~ — done (added "Self-Hosted CI Runner" section)

### Remaining Manual Steps
- ~~Create a fine-grained PAT with `Administration: Read and write` scoped to `madesroches/micromegas`~~ — done
- ~~Add the PAT as repository secret `RUNNER_PAT`~~ — done (via `gh secret set`)
- ~~Store the PAT locally for `dev_worker.py` at `~/.config/micromegas/runner-pat`~~ — done (chmod 600)
- ~~Nightly cache rotation~~ — done (use `--rotate-at 3` instead of cron)

## Implementation Notes

### Grafana E2E tests on dev-worker
The Grafana E2E test step (`python3 build/grafana_e2e_tests.py`) uses `docker compose` to start a Grafana container, which requires Docker access. Since the dev-worker container doesn't have Docker-in-Docker (by design — no `--privileged`), the E2E test step is conditionally skipped on dev-worker. The CI validation step (`python3 build/grafana_ci.py`) still runs.

### CARGO_HOME split
During image build, Rust tools are installed to the default `~/.cargo/bin` (cargo-machete, wasm-pack, wasm-bindgen-cli). At runtime, `CARGO_HOME=/cache/cargo-home` redirects registry/git caches to the Docker volume. `PATH` includes both `~/.cargo/bin` (image-installed tools) and `/cache/cargo-home/bin` (any runtime-installed binaries).

### WASM target directory
On dev-worker, the WASM job overrides `CARGO_TARGET_DIR=/cache/target-wasm` to avoid conflicts with native artifacts. On GitHub-hosted runners, a shell guard unsets the empty `CARGO_TARGET_DIR` so `build.py` uses its default behavior.

## Files Changed

**New files:**
- `docker/github-runner.Dockerfile` — runner container image
- `docker/github-runner-entrypoint.sh` — container entrypoint (token registration, runner lifecycle)
- `build/dev_worker.py` — workstation management script

**Modified files:**
- `.github/workflows/rust.yml` — added check-runner job, conditional steps, workflow_dispatch trigger, dynamic CARGO_BUILD_JOBS
- `.github/workflows/grafana-plugin.yml` — added check-runner job, conditional setup steps, E2E skipped on dev-worker
- `.github/workflows/analytics-web-app.yml` — added check-runner job, conditional setup steps
- `.github/workflows/build-skip.yml` — added check-runner stub job
- `.github/workflows/web-build-skip.yml` — added check-runner stub job
- `docker/README.md` — added github-runner image to table
- `mkdocs/docs/development/build.md` — added "Self-Hosted CI Runner" section

**Already existed (unchanged):**
- `.github/CODEOWNERS` — already had correct protection rules

## Trade-offs

### Why container-based ephemeral runner vs persistent runner agent on host?
- **Chosen:** Ephemeral container per job — clean environment, no state leakage, safe for corporate use
- **Rejected:** Persistent runner on host — accumulates state, risk of cross-job contamination, harder to reason about security

### Why check GitHub API vs timeout-based fallback?
- **Chosen:** API check from ubuntu-latest (~30s overhead) — deterministic, no stalled jobs
- **Rejected:** Start on self-hosted with short timeout — `timeout-minutes` doesn't apply to queue wait time, so an offline runner causes the job to queue for up to 6 hours

### Why owner-only gate vs all PRs?
- **Chosen:** Only repo owner's builds route to dev-worker — no untrusted code runs on personal hardware
- **Rejected:** All PRs on dev-worker — any contributor could run arbitrary code on the workstation, unacceptable in a corporate environment

### Why not use Actions Runner Controller (ARC)?
- **Chosen:** Simple Docker container + Python management script — fits single-workstation use case
- **Rejected:** ARC requires Kubernetes, overkill for one machine

### Why cache target directories too?
- **Chosen:** Cache `target-native/` and `target-wasm/` alongside `cargo-home/` — the compiled dependency artifacts (DataFusion, Arrow, etc.) take the vast majority of build time. With target caching, incremental builds should drop from ~10 minutes to ~2-3 minutes.
- **Staleness risk mitigated by:** cargo's built-in incremental compilation (recompiles when dependencies or toolchain change) plus nightly cache rotation. If anything goes wrong: `--clear-cache` nukes the single Docker volume.

### Why a single Docker volume instead of multiple?
- **Chosen:** One volume (`micromegas-build-cache`) for everything — simple mental model, one thing to delete if something goes wrong.
- **Rejected:** Separate volumes for cargo-home, target-native, target-wasm — more flexible but more to manage, more commands to clean up. Not worth the complexity.

## Documentation

- ~~`mkdocs/docs/development/build.md` — add "Self-Hosted Runner" section with setup instructions~~ — done
- ~~`docker/README.md` — add github-runner image to the service list~~ — done

## Testing Strategy

1. **Manual test:** Start dev-worker on this workstation, push a branch with workflow changes, verify:
   - Owner PR routes to dev-worker when online
   - Build completes successfully in container
   - Turning off the worker causes fallback to ubuntu-latest
   - Non-owner PR always uses ubuntu-latest
2. **Security verification:** Fork the repo from a different account, submit a PR, confirm it never touches the dev-worker
3. **Ephemeral verification:** Run two builds in sequence, confirm the container environment is fresh (no leftover processes, temp files, or workspace checkout from the previous job) while the build cache volume persists correctly
4. **Cache verification:** Run two builds, confirm second build is faster due to cargo registry cache hit
5. **Nightly rotation verification:** Run `--clear-cache`, then trigger `workflow_dispatch` on `rust.yml`, confirm the cache is fully rebuilt and subsequent builds are warm

## Resolved Decisions

All open questions have been resolved:
- **Registration tokens:** Fine-grained PAT, stored in env var or file (Component 5)
- **Multiple workers:** Any available dev-worker, no preference logic
- **Resource limits:** No limits by default — use the full workstation. Optional `--cpus` / `--memory` flags available if needed
