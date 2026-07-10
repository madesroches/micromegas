# Add Supply-Chain Gates (`cargo audit` + `cargo deny`) to Rust CI Plan

## Overview

Add pre-merge supply-chain gates to the Rust CI pipeline so problems are caught on the
PR that introduces them, rather than only after merge via Dependabot's asynchronous
security alerts. Two complementary tools, each owning a distinct concern:

- **`cargo audit`** — the RustSec **vulnerability/advisory** gate. Fast, lock-only,
  specialized. The current toolchain binary is stale (see below), so this change also
  **updates cargo-audit** to a version that can parse the modern advisory DB.
- **`cargo deny`** — **license policy + duplicate/banned crates + source allowlist**
  (`licenses`, `bans`, `sources` checks). Its `advisories` check is left OFF so it does
  not duplicate `cargo audit`.

Both checks depend only on `Cargo.lock` (and crate metadata), so they are fast and
deterministic and run on every PR. Fixes issue #1246.

## Current State

The Rust CI pipeline is driven by `build/rust_ci.py` and invoked from
`.github/workflows/rust.yml`.

`build/rust_ci.py:10-17` — `run_native()` runs four steps in order:

```python
("Formatting Check", "cargo fmt --check", None),
("Clippy Linting", "cargo clippy --workspace -- -D warnings", None),
("Unused Dependencies Check", "cargo machete", None),
("Running Tests", "cargo test", None),
```

Each step is a `(name, cmd, cwd)` tuple; `cwd=None` means run from `rust/` (see
`rust_command.py:16`, default `cwd=rust_root`). `run_command` shells out with
`check=True`, so a non-zero exit fails the pipeline. There is no advisory, license, or
supply-chain step.

`.github/workflows/rust.yml`:
- The `native` job (lines 28-73) installs `cargo-machete` (lines 60-62) only on the
  GitHub-hosted runner (`if: needs.check-runner.outputs.runner == 'ubuntu-latest'`) —
  the `dev-worker` self-hosted image is expected to have such tools pre-baked. It then
  runs `./build/rust_ci.py native`.
- `check-runner.yml` selects between `dev-worker` (trusted authors, when online) and
  `ubuntu-latest` (fail-safe default).

`CONTRIBUTING.md` has a **CI Tools** section (around line 60) documenting that
`cargo-machete` must be installed to run the pipeline locally. There is no `deny.toml`
or `.cargo/audit.toml` anywhere in the repo, and no existing `cargo audit`/`cargo deny`
usage (only a passing mention in a completed study doc). Security alerts fire post-merge
only.

**Dependency-tree facts** (gathered via `cargo metadata` / `Cargo.lock` on
`rust/Cargo.lock`):
- **No git dependencies and no non-crates.io registries.** The `sources` gate can
  therefore require crates.io exclusively.
- License SPDX strings observed across the tree (counts approximate):
  `MIT OR Apache-2.0` (293), `MIT` (98), `Apache-2.0` (97), `Unicode-3.0` (18),
  `BSD-3-Clause` (7), `Apache-2.0 WITH LLVM-exception` (variants), `Unlicense` (in OR),
  `Zlib` (3), `ISC` (3), `BSD-2-Clause`, `BSL-1.0`, `0BSD`, `CC0-1.0`, `MIT-0`,
  `bzip2-1.0.6`, `CDLA-Permissive-2.0`, and one `MPL-2.0`. `LGPL-2.1-or-later` appears
  **only** inside an `OR` expression alongside MIT/Apache, so it never needs allowing.
  Several crates use the deprecated non-SPDX slash form (`MIT/Apache-2.0`) — modern
  cargo-deny parses these but may warn; not a blocker.

## Design

### Why both tools instead of `cargo deny` alone

`cargo deny` can run an `advisories` check too, so in principle it could replace
`cargo audit`. We keep both because:
- `cargo audit` is the reference RustSec implementation, updates its advisory DB on
  every run, and gives the cleanest vuln-only signal and ignore ergonomics.
- Splitting concerns keeps each gate's failures unambiguous ("vuln" vs "license/policy")
  and lets us tune them independently.
- The issue (#1246) explicitly requested evaluating `cargo deny`; the user asked to add
  it now and to update cargo-audit — this design does both without redundant advisory
  scanning (cargo-deny's `advisories` check is disabled).

### `cargo audit` — vulnerability gate (updated)

Default exit behavior already matches the staged policy the issue wants:
- **Vulnerabilities** → non-zero exit → **fails the build**.
- **Warnings** (unmaintained, unsound, yanked) → reported but **exit 0** by default.

So the step is plain `cargo audit` (no `--deny warnings`) initially; tighten later with
`--deny warnings` once the baseline is clean.

**Baseline (measured with cargo-audit 0.22.2 against the committed lock):** 4
vulnerabilities + 3 non-fatal warnings. To land the gate green the implementation must
resolve each — see the table under Resolved Findings. In short: bump `crossbeam-epoch`
(fix available, dev-only), and add documented ignores for the two `quick-xml` advisories
(pinned transitively by `object_store`) and the `rsa` Marvin advisory (no fix exists).

**Tool update (required).** The RustSec advisory DB now contains CVSS 4.0 entries, and
older `cargo-audit` releases abort loading the DB on them. Verified locally:
`cargo-audit 0.21.2` fails with

```
error loading advisory database: ... unsupported CVSS version: 4.0
  (parsing crates/lz4_flex/RUSTSEC-2026-0041.md)
```

`cargo install cargo-audit --locked` fetches the latest published release, which
supports CVSS 4.0. **Confirmed:** 0.21.2 is known-bad; **0.22.2 works** (loads the DB
and reports findings). To guarantee a too-old cached/baked binary is replaced rather
than reused, pin the floor: `cargo install cargo-audit --locked --version '^0.22'`.

**Ignore config** — `rust/.cargo/audit.toml` (auto-discovered; the pipeline runs from
`rust/`):

```toml
# cargo-audit configuration — see https://docs.rs/cargo-audit
#
# Add an advisory ID here ONLY when there is no fixed version available yet.
# Every entry MUST carry a comment: the advisory URL, why it does not apply
# (or cannot be fixed yet), and a condition for removal. Revisit on each
# dependency bump — remove the ignore as soon as a fix ships.
[advisories]
ignore = [
    # rsa 0.9.10 (via jsonwebtoken -> micromegas-auth): Marvin timing
    # side-channel. NO fixed upstream version exists as of this writing.
    # Revisit when the `rsa` crate ships a fix. Tracked in #NNNN.
    "RUSTSEC-2023-0071",

    # quick-xml 0.39.4 (via object_store 0.13.2): DoS in XML parsing of
    # object-store list responses. Fix is quick-xml >=0.41.0, but
    # object_store 0.13.2 pins ^0.39 and object_store is in turn pinned by
    # datafusion 54. Cannot bump within semver today. Remove once datafusion
    # bumps object_store to a release that pulls quick-xml >=0.41. Tracked in #NNNN.
    "RUSTSEC-2026-0194",
    "RUSTSEC-2026-0195",
]
```

`crossbeam-epoch`'s advisory (RUSTSEC-2026-0204) is deliberately **not** ignored — it is
fixed by a dependency bump (below), not an ignore.

### `cargo deny` — license + bans + sources gate

New `rust/deny.toml` (config schema version 2). Run only the non-advisory checks so it
does not duplicate cargo-audit:

```
cargo deny check licenses bans sources
```

Proposed initial `rust/deny.toml`:

```toml
# cargo-deny configuration — https://embarkstudios.github.io/cargo-deny/
# Advisory scanning is intentionally handled by `cargo audit`, not here.

[licenses]
# Permissive licenses actually present in the dependency tree. Add a new
# entry (with a one-line justification) only after reviewing the crate.
allow = [
    "MIT",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "Unicode-3.0",
    "Unlicense",
    "BSL-1.0",
    "0BSD",
    "CC0-1.0",
    "MIT-0",
    "bzip2-1.0.6",
    "CDLA-Permissive-2.0",
    # MPL-2.0: `colored` 3.1.1 (via micromegas-telemetry-sink). Weak,
    # file-level copyleft — no obligations when consumed unmodified as a
    # library dependency, so accepted. Verified `cargo deny check licenses`
    # passes with this entry.
    "MPL-2.0",
]
confidence-threshold = 0.8
# Per-crate license clarifications/exceptions go here if a crate's SPDX
# metadata is missing or wrong:
# [[licenses.exceptions]]
# name = "some-crate"
# allow = ["..."]

[bans]
# Start permissive: a DataFusion/Arrow tree has many legitimate duplicate
# versions. Surface them without failing the build; tighten to "deny" for
# specific crates later via `skip`/`deny` once triaged.
multiple-versions = "warn"
wildcards = "warn"

[sources]
# Verified: the tree has no git deps and no non-crates.io registries.
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
```

Notes:
- `multiple-versions = "warn"` (not `deny`) avoids a wall of failures on a large
  Arrow/DataFusion workspace; it can be tightened incrementally.
- The `sources` gate is safe to set to `deny` immediately because the tree is
  crates.io-only today; if a git dependency is ever added, the gate forces an explicit,
  reviewed allowlist entry.

### CI wiring

**1. Pipeline steps** — add two steps to `run_native()` in `build/rust_ci.py`, after the
unused-deps check and before tests (fail fast on cheap, lock-only checks):

```python
("Advisory Audit", "cargo audit", None),
("License & Supply-Chain (deny)", "cargo deny check licenses bans sources", None),
```

**2. Tool install** — add install steps to the `native` job in
`.github/workflows/rust.yml`, next to `Install cargo-machete`. Do **not** guard with
`if: ... == 'ubuntu-latest'`, because the `dev-worker` image is not guaranteed to have
these binaries (and its `cargo-audit`, if present, may be too old). `cargo install` is a
fast no-op when the pinned version is already present:

```yaml
- name: Install cargo-audit
  run: cargo install cargo-audit --locked --version '^0.22'
- name: Install cargo-deny
  run: cargo install cargo-deny --locked
```

(Optional follow-up: bake both into the `dev-worker` image and re-add the
`ubuntu-latest` guard for symmetry with machete.)

`cargo audit` fetches the advisory DB over the network; both runner types have network
access, so no extra plumbing is needed. `cargo deny`'s license/bans/sources checks are
offline (metadata + lock only).

## Implementation Steps

1. **`build/rust_ci.py`** — add the two steps (`Advisory Audit`, then
   `License & Supply-Chain (deny)`) to `run_native()`, after `Unused Dependencies Check`
   and before `Running Tests`.
2. **Resolve the current baseline** so the gate lands green:
   - Run `cargo update -p crossbeam-epoch` (0.9.18 → 0.9.20) to fix RUSTSEC-2026-0204.
   - Create `rust/.cargo/audit.toml` with the documented ignores for `rsa`
     (RUSTSEC-2023-0071, no fix) and the two `object_store`-pinned `quick-xml`
     advisories (RUSTSEC-2026-0194/0195), each with a tracking-issue reference.
   - Open a tracking issue for the ignored advisories (referenced from the config).
3. **`rust/deny.toml`** — create with the `licenses`/`bans`/`sources` config above.
4. **`.github/workflows/rust.yml`** — add `Install cargo-audit` and `Install cargo-deny`
   steps to the `native` job (no `ubuntu-latest` guard; add cargo-audit version floor
   once confirmed).
5. **`CONTRIBUTING.md`** — extend **CI Tools**: install both
   (`cargo install cargo-audit --locked`, `cargo install cargo-deny --locked`), note the
   pipeline runs `cargo audit` and `cargo deny check licenses bans sources` from `rust/`,
   how to run each standalone, and how to add a documented advisory ignore / license
   allow entry.
6. **Local verification** — install a current cargo-audit and cargo-deny, then run both
   against the committed `Cargo.lock`/tree and triage results (see Open Questions).

## Files to Modify

- `build/rust_ci.py` — add the two pipeline steps.
- `.github/workflows/rust.yml` — add cargo-audit and cargo-deny install steps.
- `CONTRIBUTING.md` — document local install/run and the ignore/allow mechanisms.
- `rust/.cargo/audit.toml` — **new** — advisory ignore list.
- `rust/deny.toml` — **new** — license/bans/sources policy.

## Trade-offs

- **Two tools vs. `cargo deny` alone**: cargo-deny could also do advisories, but keeping
  cargo-audit as the dedicated vuln gate gives clearer signals and the canonical RustSec
  behavior; cargo-deny's `advisories` check is disabled to avoid double-reporting the
  same CVEs. Cost: two binaries to install/maintain instead of one.
- **`bans.multiple-versions = "warn"` not `"deny"`**: a strict duplicate-version gate on
  an Arrow/DataFusion tree would fail immediately and require a large `skip` list with no
  security benefit. Start with visibility; tighten selectively.
- **`sources` set to `deny` now**: safe because the tree is crates.io-only today, and it
  makes any future git/registry dependency a deliberate, reviewed decision.
- **Install-on-run vs. cache/bake**: mirrors the existing `cargo-machete` approach for
  consistency; adds ~1–2 min first-time compile per tool on a cold GitHub-hosted runner.
  Baking into `dev-worker` is a noted optional follow-up.
- **Default audit policy (warn on unmaintained/yanked)**: avoids blocking PRs on
  no-fix-available noise; tighten with `--deny warnings` once the baseline is clean.

## Documentation

- `CONTRIBUTING.md` — **CI Tools** section (primary; step 5).
- No `mkdocs/` site page covers CI tooling today, so no docs-site change is required.
  Consider a one-line mention on a future "Development/CI" docs page if one is created.

## Testing Strategy

- Install current tooling locally (cargo-audit ≥0.22, cargo-deny), then run each gate
  against the committed lock/tree:
  - `cd rust && cargo audit` → expect exit 0 **after** the crossbeam bump and the three
    documented ignores are in place (it fails today with 4 vulns — see Resolved
    Findings).
  - `cd rust && cargo deny check licenses bans sources` → already passes (exit 0) with
    the proposed `deny.toml`, though the output is noisy: expect ~9 wildcard warnings,
    ~40 duplicate-version warnings, and 8 non-fatal unresolved-workspace-dependency
    diagnostics (see Resolved Findings item 1) — none of these are failures.
- Run the full pipeline locally: `python3 build/rust_ci.py native` and confirm both new
  steps appear and pass.
- Negative checks (manual, throwaway, then revert):
  - Add a known-vulnerable crate/version → `cargo audit` exits non-zero.
  - Add a crate under a non-allowed license → `cargo deny check licenses` fails.
- CI: confirm the `native` job installs both tools and both steps run green on the PR.

## Resolved Findings

All three original open questions were resolved by installing the tooling
(cargo-audit 0.22.2, cargo-deny 0.20.2) and running it against the committed lock/tree.

**1. Does the tree pass today?** `cargo deny check licenses bans sources` **passes**
(exit 0) with the proposed `deny.toml`, but the output is noisy, not clean: ~9
`warning[wildcard]` entries across workspace crates (analytics-web-srv, flight-sql-srv,
http-gateway, telemetry-ingestion-srv, telemetry-maintenance-srv, uri-handler,
write-perfetto, micromegas-monolith, micromegas-object-cache-srv — all
workspace-inherited deps, non-fatal under `wildcards = "warn"`), ~40 `warning[duplicate]`
entries (expected on an Arrow/DataFusion tree under `multiple-versions = "warn"`), and 8
non-fatal `bug[unresolved-workspace-dependency]` diagnostics. None of this fails the
build — it is the expected shape of the passing output, not a sign of breakage.
`cargo audit` currently **fails** with 4 vulnerabilities (plus 3 non-fatal warnings),
each with a determined resolution:

| Advisory | Crate | Path | Fix available? | Resolution |
|---|---|---|---|---|
| RUSTSEC-2026-0204 | crossbeam-epoch 0.9.18 | dev-only, via `criterion` benches | Yes (≥0.9.20) | `cargo update -p crossbeam-epoch` |
| RUSTSEC-2026-0194 (high) | quick-xml 0.39.4 | via `object_store` 0.13.2 | ≥0.41.0, but **pinned** (object_store ^0.39, gated by datafusion 54) | documented ignore + tracking issue |
| RUSTSEC-2026-0195 (high) | quick-xml 0.39.4 | via `object_store` 0.13.2 | same as above | documented ignore + tracking issue |
| RUSTSEC-2023-0071 (med) | rsa 0.9.10 | via `jsonwebtoken` → `micromegas-auth` | **No fix exists** | documented ignore + tracking issue |

Non-fatal warnings (exit 0 under plain `cargo audit`; no action required now, candidates
for follow-up bumps): `paste` (unmaintained, RUSTSEC-2024-0436), `proc-macro-error2`
(unmaintained, RUSTSEC-2026-0173), `anyhow` 1.0.102 (unsound `downcast_mut`,
RUSTSEC-2026-0190).

**2. Allow `MPL-2.0`?** Yes. The only MPL-2.0 crate is `colored` 3.1.1 (terminal colors,
via `micromegas-telemetry-sink`). MPL-2.0 is file-level weak copyleft with no obligations
when consumed unmodified as a library dependency. `cargo deny check licenses` passes with
it allowed.

**3. cargo-audit version floor.** Confirmed: 0.21.2 is broken on the current DB (CVSS
4.0); **0.22.2 works**. Install step pins `--version '^0.22'`.

## Open Questions

- None blocking. The one remaining decision is bookkeeping: the tracking-issue number to
  reference from the ignored advisories in `rust/.cargo/audit.toml` (create during
  implementation).
