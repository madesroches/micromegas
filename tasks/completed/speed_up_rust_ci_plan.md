# Speed Up Rust CI on the Self-Hosted Worker Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1610

## Overview

The Rust CI pipeline (`build/rust_ci.py`, invoked from `.github/workflows/rust.yml`) currently
takes 8-15 minutes for a normal `rust.yml` run and closer to 27 minutes on a dependency bump. A
prior evaluation concluded that migrating to Bazel is not worth it (the self-hosted `dev-worker`
already provides persistent-cache builds, which was Bazel's main selling point here). This plan
implements the three cheaper improvements from the issue that target the same wall time without
a build-system migration:

1. Switch `cargo test` to `cargo-nextest` for parallel, process-per-test execution.
2. Bake `cargo-audit` and `cargo-deny` into the `dev-worker` image, matching the existing
   `cargo-machete` convention, so they don't need a same-cost-as-fresh-install check on every run.
3. Run the independent, non-compiling checks (`cargo machete`, `cargo audit`, `cargo deny`, both
   trees) concurrently with the clippy/test build instead of serially before it.

The fourth proposed change (splitting the `native` job into separate lint+audit and test jobs) is
explicitly blocked by `build/dev_worker.py`'s one-worker-per-workstation constraint and is treated
as out of scope — see Trade-offs.

## Current State

`build/rust_ci.py:10-29` (`run_native()`) runs eight steps strictly serially via `_run_steps`
(`build/rust_ci.py:43-58`), which loops over a flat `(name, cmd, cwd)` list and calls
`run_command` (`build/rust_command.py:16-19`, `subprocess.run(cmd, shell=True, cwd=cwd,
check=True)`, output inherited/streamed live) for each one in turn:

```
Formatting Check → Clippy Linting → Unused Dependencies Check (machete) →
Advisory Audit (cargo audit) → License & Supply-Chain (deny) →
Advisory Audit (datafusion-wasm) → License & Supply-Chain (deny, datafusion-wasm) →
Running Tests (cargo test)
```

`.github/workflows/rust.yml`'s `native` job:
- Installs `cargo-machete` (line 60-62) only `if: needs.check-runner.outputs.runner ==
  'ubuntu-latest'` — the `dev-worker` image already bakes it in (see below), so the install is
  skipped there.
- Installs `cargo-audit` (line 64-67) and `cargo-deny` (line 69-72) unconditionally, on both
  runner types, with no matching `if:` guard.

`docker/github-runner.Dockerfile:119-123` bakes `cargo-machete` and `wasm-pack` into the
`dev-worker` image at build time (before `ENV CARGO_HOME=/cache/rust/cargo-home` is set at line
142, so they land in the image's `~/.cargo/bin`, not the per-workstation cache volume). It does
**not** bake in `cargo-audit`, `cargo-deny`, or a nextest-equivalent.

`build/dev_worker.py:44-45` documents the cache-volume constraint that blocks job-splitting:
`CACHE_VOLUME` layout "[a]ssumes a single worker per workstation; multiple concurrent workers
would corrupt cargo's registry/target locks."

**Doctests.** 23 files under `rust/` carry `` ```rust `` fenced doc-comment examples (e.g.
`rust/auth/src/oauth_state.rs`), several with real `assert_eq!` checks that `cargo test`
currently exercises as part of its default target set. `cargo nextest run` does not execute
doctests at all (a documented nextest limitation — it only runs the standard `#[test]`/library
test binaries, not `rustdoc --test`), so switching the "Running Tests" step to nextest outright
would silently stop verifying these.

**`#[serial]` tests.** The workspace uses `serial_test`'s `#[serial]` attribute in several test
files (`ingestion/tests/data_lake_config_tests.rs`, `auth/src/env.rs`,
`object-cache/tests/foyer_backend_tests.rs`, others) to serialize tests that mutate process-wide
state (unsafe `std::env::set_var`, shared tracing dispatch) — safe under `cargo test` because all
tests in a binary share one process. `cargo-nextest`'s core design runs every test in its own
process, so this class of test is unaffected by the switch: each test already gets an isolated
process, a *stronger* guarantee than `serial_test`'s same-process mutex provides. The attribute
becomes a redundant no-op rather than a compatibility problem — no code changes needed.

No `[[test]]` target in the workspace sets `harness = false` (only `[[bench]]` targets in
`analytics/Cargo.toml` and `tracing/Cargo.toml` do, which neither `cargo test` nor
`cargo nextest run` executes by default), so there is no custom-harness incompatibility to work
around.

## Design

### 1. `cargo-nextest` for test execution

Add a `"Doc Tests"` step (`cargo test --doc`) immediately after switching `"Running Tests"` to
`cargo nextest run`, so doctest coverage is preserved explicitly rather than silently dropped.
`cargo test --doc` only compiles/runs doc examples, not the full test suite, so it stays fast.

### 2. Bake `cargo-audit`/`cargo-deny` into the `dev-worker` image

Today, `cargo install cargo-audit --locked --version '^0.22'` and `cargo install cargo-deny
--locked` run on every `native` job, including on `dev-worker`. Because `CARGO_HOME` there is the
persistent `/cache/rust/cargo-home` volume (`docker/github-runner.Dockerfile:142-143`,
`build/dev_worker.py`'s `micromegas-runner-cache` mount), `cargo install` on an exact version
match already no-ops without recompiling — so the *measured* per-run cost of this step on a warm
dev-worker may already be near zero. The issue explicitly asks to measure before changing.

This plan bakes both tools into the image anyway, gated on the measurement in Implementation Step
2 confirming it's worth doing: it makes `dev-worker` self-contained the same way `cargo-machete`
already is (no dependency on a warm cache surviving; a `--cleanup`'d or fresh cache volume no
longer means re-resolving/reinstalling audit/deny on the next job), and it removes even a no-op
version check plus its registry-index touch from the hot path — consistent with, not a deviation
from, the existing `cargo-machete` pattern in the same Dockerfile and workflow.

### 3. Parallelize the independent checks

`cargo machete`, `cargo audit` (both trees), and `cargo deny check ...` (both trees) read only
`Cargo.lock`/crate metadata — they don't invoke `cargo build` and don't need the compiled
workspace, so they don't contend with `cargo fmt --check` / `cargo clippy` / `cargo nextest run`
/ `cargo test --doc` for the target-directory build lock. Running the five metadata-only checks
concurrently with the four build-dependent steps overlaps their wall time instead of paying for
both in sequence.

One thing the concurrent steps *do* contend on: when the pinned toolchain from
`rust/rust-toolchain.toml` isn't installed yet, every cargo invocation triggers a rustup
auto-install, and six of those at once clobber each other's partial downloads in
`~/.rustup/downloads` (`could not rename 'downloaded' file`). `_run_steps` therefore runs
`cargo --version` once per distinct cargo working directory, serially, before dispatching the
pool.

Split `run_native()`'s step list into two groups and run them with a small parallel harness:

```python
# build/rust_ci.py
from concurrent.futures import ThreadPoolExecutor
from rust_command import run_command, run_captured, show_disk_space

def run_native():
    sequential_steps = [
        ("Formatting Check", "cargo fmt --check", None),
        ("Clippy Linting", "cargo clippy --workspace -- -D warnings", None),
        ("Running Tests", "cargo nextest run", None),
        ("Doc Tests", "cargo test --doc", None),
    ]
    parallel_steps = [
        ("Unused Dependencies Check", "cargo machete", None),
        ("Advisory Audit", "cargo audit", None),
        ("License & Supply-Chain (deny)", "cargo deny check licenses bans sources", None),
        ("Advisory Audit (datafusion-wasm)", "cargo audit", wasm_crate),
        (
            "License & Supply-Chain (deny, datafusion-wasm)",
            "cargo deny --config ../deny.toml check licenses bans sources --allow unnecessary-skip",
            wasm_crate,
        ),
    ]
    _run_steps("Native", sequential_steps, parallel_steps)


def run_wasm():
    steps = [
        ("WASM Dependency Version Check", "python3 build/check_wasm_deps.py", repo_root),
        ("WASM Formatting Check", "cargo fmt --check", wasm_crate),
        ("WASM Clippy", "cargo clippy --target wasm32-unknown-unknown -- -D warnings", wasm_crate),
        ("WASM Tests", "python3 build.py --test", wasm_crate),
        ("WASM Bindings Freshness Check", "python3 build.py --check", wasm_crate),
    ]
    _run_steps("WASM", steps)  # parallel_steps defaults to [] — unchanged behavior


def _run_parallel_step(name, cmd, cwd):
    kwargs = {"cwd": cwd} if cwd else {}
    result = run_captured(cmd, **kwargs)
    status = "PASSED" if result.returncode == 0 else "FAILED"
    print(f"\n{'=' * 60}\n[parallel] {status}: {name}\n{'=' * 60}\n{result.stdout}{result.stderr}")
    return name, result.returncode == 0


def _run_steps(label, sequential_steps, parallel_steps=None):
    parallel_steps = parallel_steps or []
    total = len(sequential_steps) + len(parallel_steps)
    print("=" * 60)
    print(f"Starting {label} CI Pipeline")
    print("=" * 60)
    show_disk_space()

    with ThreadPoolExecutor(max_workers=max(len(parallel_steps), 1)) as pool:
        futures = [pool.submit(_run_parallel_step, *step) for step in parallel_steps]

        for i, (name, cmd, cwd) in enumerate(sequential_steps, 1):
            print(f"\n{'=' * 60}")
            print(f"Step {i}/{total}: {name}")
            print("=" * 60)
            kwargs = {"cwd": cwd} if cwd else {}
            run_command(cmd, **kwargs)

        results = [f.result() for f in futures]

    failed = [name for name, ok in results if not ok]
    if failed:
        print(f"\nParallel steps failed: {', '.join(failed)}")
        sys.exit(1)

    print(f"\n{'=' * 60}")
    print(f"{label} CI steps completed successfully!")
    print("=" * 60)
    show_disk_space()
```

`run_captured` is a new sibling to `run_command` in `build/rust_command.py` that captures combined
output instead of streaming it live and returns the `CompletedProcess` instead of raising, so the
caller can report pass/fail without interleaving output from concurrently-running subprocesses on
stdout:

```python
# build/rust_command.py
def run_captured(cmd, cwd=rust_root):
    """Like run_command, but captures output and returns the result instead of raising."""
    print("cmd=", cmd, "cwd=", cwd)
    return subprocess.run(cmd, shell=True, cwd=cwd, capture_output=True, text=True)
```

Each parallel step's full output is printed as one block as soon as that step finishes (not
buffered until the very end), so a failure is visible in the log as soon as it happens even though
the script doesn't exit until the sequential steps also finish.

## Implementation Steps

1. **`build/rust_command.py`** — add `run_captured` (above).
2. **Measure, then bake `cargo-audit`/`cargo-deny` into the image**:
   - On a warm `dev-worker` (persistent `CARGO_HOME`), time
     `cargo install cargo-audit --locked --version '^0.22'` and
     `cargo install cargo-deny --locked` back to back with an already-current install.
   - Add both, plus `cargo-nextest`, to `docker/github-runner.Dockerfile:119-123`'s
     `cargo install` chain (before `ENV CARGO_HOME` is overridden, alongside `cargo-machete` and
     `wasm-pack`):
     ```dockerfile
     RUN rustup target add wasm32-unknown-unknown \
         && rustup target add x86_64-pc-windows-gnu \
         && cargo install cargo-machete \
         && cargo install cargo-nextest --locked \
         && cargo install cargo-audit --locked --version '^0.22' \
         && cargo install cargo-deny --locked \
         && cargo install wasm-pack
     ```
   - In `.github/workflows/rust.yml`, add `if: needs.check-runner.outputs.runner ==
     'ubuntu-latest'` to the existing `Install cargo-audit` and `Install cargo-deny` steps (matching
     `Install cargo-machete`'s guard) and add a new, identically-guarded `Install cargo-nextest`
     step:
     ```yaml
     - name: Install cargo-nextest
       if: needs.check-runner.outputs.runner == 'ubuntu-latest'
       run: cargo install cargo-nextest --locked
     ```
   - Rebuild and redeploy the `dev-worker` image (`python3 build/dev_worker.py --build-image`).
3. **`build/rust_ci.py`** — apply the Design section's restructuring: split `run_native()`'s steps
   into `sequential_steps`/`parallel_steps`, replace `"Running Tests"`'s `cargo test` with
   `cargo nextest run`, add the `"Doc Tests"` (`cargo test --doc`) step, and rewrite `_run_steps`
   to take both groups (defaulting `parallel_steps` to `[]` so `run_wasm()`'s existing call site
   needs no change).
4. **`build/test_rust_ci.py`** (new) — unit test `_run_steps`'s orchestration logic in isolation,
   using trivial constructed commands (`sys.executable -c "..."` with exit 0/1), not real
   cargo/audit/deny invocations:
   - A parallel-step failure causes the process to exit non-zero even when all sequential steps
     pass.
   - All sequential steps still run, in order, when every parallel step passes.
   - A sequential-step failure propagates (existing `run_command`/`check=True` behavior).
   - `run_wasm()`'s existing single-list call shape (`_run_steps(label, steps)`, no
     `parallel_steps`) still runs every step sequentially.
5. **`.github/workflows/rust.yml`** — add `build/test_rust_ci.py` to the `pull_request`/`push`
   `paths:` filters (alongside the existing `build/rust_ci.py` and `build/rust_command.py`
   entries), and add a step to the `native` job, before "Run native CI", that installs pytest and
   runs it:
     ```yaml
     - name: Test rust_ci.py orchestration
       run: |
         python3 -m pip install --quiet pytest
         python3 -m pytest build/test_rust_ci.py
     ```
6. **`CONTRIBUTING.md`** — extend the **CI Tools** section: add `cargo install cargo-nextest
   --locked` to the local install list, and note that the pipeline runs `cargo nextest run` +
   `cargo test --doc` (not plain `cargo test`) and that the machete/audit/deny checks now run
   concurrently with the build/test steps rather than before them. Also update the stale
   `# Runs format check, clippy, and tests` comment on the `python3 build/rust_ci.py` line in the
   **Rust Workspace (Primary)** section (line ~239) — see Decisions.

## Files to Modify

- `build/rust_command.py` — add `run_captured`.
- `build/rust_ci.py` — split `run_native()` into sequential/parallel step groups; switch to
  `cargo nextest run` + `cargo test --doc`; rewrite `_run_steps` to run the parallel group
  concurrently with the sequential group.
- `build/test_rust_ci.py` — **new** — unit tests for `_run_steps`'s orchestration logic.
- `docker/github-runner.Dockerfile` — bake in `cargo-nextest`, `cargo-audit`, `cargo-deny`.
- `.github/workflows/rust.yml` — guard `Install cargo-audit`/`Install cargo-deny` to
  `ubuntu-latest`; add a guarded `Install cargo-nextest` step; add a pytest step for
  `build/test_rust_ci.py`; add that file to the trigger `paths:` filters.
- `CONTRIBUTING.md` — document the nextest switch and the new concurrent-checks behavior; correct
  the stale `# Runs format check, clippy, and tests` comment in the Rust Workspace section.

## Trade-offs

- **Losing "fail fast" on cheap checks.** Today, a `cargo machete`/`audit`/`deny` failure aborts
  the pipeline in seconds, before the multi-minute clippy/test build even starts. Running them
  concurrently means that benefit disappears — a machete failure now surfaces only after (or
  alongside) the build finishes, not instead of it. This plan accepts that trade because: the
  parallel checks are all sub-minute compared to the multi-minute build they'd otherwise gate, so
  the absolute delay to see a failure is bounded by the sequential group's own time either way; and
  each parallel step's result prints as soon as it completes (not batched to the end), so the
  failure is visible in the log promptly even though the script itself doesn't exit early.
- **Splitting `native` into separate jobs (issue's 4th proposed change) — deferred, not
  implemented here.** The issue itself flags this as blocked on `build/dev_worker.py`'s
  one-worker-per-workstation constraint: concurrent workers on the same workstation would corrupt
  the shared cargo registry/target locks in `/cache`. Lifting that needs a per-worker cache volume
  (or another isolation scheme), which is its own infra change with its own risk (cache
  duplication/coherency across workers) — out of scope for a CI-script/Dockerfile change. Tracked
  as follow-up work, not an open question, since the issue already states the blocker.
- **Buffered (not streamed) output for parallel steps.** Streaming five concurrent subprocesses'
  output live to one log would interleave lines unreadably. Printing each step's full output as a
  block right when it finishes is less real-time than the current live stream but stays
  attributable and still surfaces failures promptly (see above).
- **Baking audit/deny into the image versus leaving the runtime install.** The issue asks to
  measure first because the warm-cache case may already be free. This plan bakes them in
  regardless of the measured delta (Implementation Step 2) because it matches the already-accepted
  `cargo-machete` precedent in the same files, and removes a dependency on the cache volume surviving
  (a `--cleanup`'d cache shouldn't force re-resolving these two tools) at effectively zero
  incremental cost — not because the measurement itself is expected to show a large win.

## Decisions

- Doctests are preserved via an explicit `cargo test --doc` step rather than dropped, since
  `cargo-nextest` never runs them and several are real, assertion-bearing examples.
- `#[serial]`-marked tests need no changes: `cargo-nextest`'s one-process-per-test model already
  gives each test isolated process-wide state, a strictly stronger guarantee than `serial_test`'s
  same-process mutex, so nothing breaks — see Current State.
- The job-splitting proposal (issue's 4th checkbox) is out of scope for this plan; it needs the
  per-worker cache volume infra change the issue itself names as the blocker.
- `CONTRIBUTING.md`'s `# Runs format check, clippy, and tests` comment is replaced with
  `# Runs the full native CI pipeline` rather than an updated step enumeration, so it doesn't go
  stale again the next time the pipeline's step list changes.

## Documentation

- `CONTRIBUTING.md`'s **CI Tools** section — add `cargo-nextest` to the local install list and
  describe the nextest/doctest split and the now-concurrent independent checks (Implementation
  Step 6). The same file's **Rust Workspace (Primary)** section also carries a
  `# Runs format check, clippy, and tests` comment next to the `python3 build/rust_ci.py` command
  line (~239) that is already stale today (it omits machete/audit/deny) and would drift further
  after this plan; Implementation Step 6 generalizes it instead (see Decisions). No `mkdocs/` page
  documents the pipeline's internal step list or ordering, so none needs updating.

## Testing Strategy

- **`build/test_rust_ci.py`** (new, no-DB, no-network unit test) — exercises `_run_steps`'s
  orchestration logic directly with constructed trivial commands (see Implementation Step 4):
  parallel-failure detection, sequential-step ordering, sequential-failure propagation, and the
  no-`parallel_steps` call shape `run_wasm()` still uses. This is genuine branching logic
  (success/failure combinations across two step groups) reachable with constructed inputs, so it
  gets a unit test rather than being left to manual verification.
- **Full local pipeline run** (manual — see below) exercises the real `cargo nextest run`,
  `cargo test --doc`, and the five parallel checks together against the actual workspace; this is
  wiring/integration behavior (does the real `cargo-nextest` binary exist and behave, does the real
  parallel execution avoid corrupting `Cargo.lock`/target-dir state) that a unit test with fake
  commands can't reach, and a breakage here would be immediately obvious the next time anyone runs
  the pipeline — exactly the profile for a manual check instead of an automated one.
- **CI itself** is the other tier of coverage: the modified `.github/workflows/rust.yml` runs on
  the PR that carries this change, on both `ubuntu-latest` (fallback path, exercises the new
  `Install cargo-nextest` step) and `dev-worker` if online (exercises the rebuilt image).

## Manual Verification

1. **Measure the audit/deny install cost before baking it in** (Design section 2, Implementation
   Step 2) — on a warm `dev-worker`, time `cargo install cargo-audit --locked --version '^0.22'`
   and `cargo install cargo-deny --locked` when the pinned version is already installed. Expected:
   near-instant no-op either way; proceed with baking them into the image regardless (see
   Trade-offs), but record the measured numbers in the PR description since the issue explicitly
   asked for them.
2. **Doctest parity** — before switching, run `cd rust && cargo test --doc` alone and note the
   number of doctests that pass. After the change, confirm `cargo nextest run` reports zero
   doctests (expected — it doesn't run them) and the new `cargo test --doc` step reports the same
   count passing as before. Not automatable without duplicating the doctest runner itself; a count
   mismatch would be immediately visible in the step's own output.
3. **Full local pipeline** — `python3 build/rust_ci.py native` from the repo root. Confirm: all
   nine steps (4 sequential + 5 parallel) report, the parallel steps' output blocks appear as they
   complete rather than all at the end, and the script exits 0 only when everything passed.
4. **Parallel-failure visibility** — temporarily break one parallel check (e.g., add an
   intentionally unused dependency to trigger `cargo machete`) and re-run
   `python3 build/rust_ci.py native` locally; confirm the failure is reported and the process exits
   non-zero even though the sequential steps (fmt/clippy/tests) still ran to completion. Revert the
   intentional breakage afterward.
5. **Rebuilt `dev-worker` image** — `python3 build/dev_worker.py --build-image`, then start the
   worker and push a trivial commit; confirm the `native` job routes to `dev-worker`
   (`check-runner` output) and completes without any `cargo install` steps running (they're now
   guarded to `ubuntu-latest` only).

## Open Questions

None blocking. The audit/deny install-cost measurement (Manual Verification step 1) is a
number to record during implementation, not a decision that needs to happen before it — this plan
already commits to baking both tools into the image either way (see Trade-offs).
