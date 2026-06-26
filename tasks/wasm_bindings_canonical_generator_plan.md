# DataFusion WASM Bindings: Single Canonical Generator Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1169

## Overview

The checked-in DataFusion WASM bindings under `analytics-web-app/src/lib/datafusion-wasm/`
(`micromegas_datafusion_wasm.js`, `.d.ts`, `package.json`) churn in `git diff` even when the
WASM source crate has not changed. The cause is that **two different generators** —
`build.py` (which runs `wasm-bindgen --target web`) and `wasm-pack build` — produce
structurally different output for the same artifacts, and whichever a developer last ran wins.
This plan makes `build.py` the single canonical generator, gives `package.json` one intentional
shape, removes the stray `wasm-pack` artifacts, and documents the build path so regeneration is
byte-reproducible.

## Current State

### The two build paths

1. **`rust/datafusion-wasm/build.py`** (`build()`):
   - `cargo build --target wasm32-unknown-unknown --release`
   - `wasm-bindgen <wasm> --out-dir pkg --target web`
   - optional `wasm-opt`
   - copies everything from `pkg/` to the output dir
   - **overwrites `package.json`** with a hardcoded dict (`build.py:21`):
     ```python
     WASM_PACKAGE_JSON = {
         "name": "micromegas-datafusion-wasm",
         "version": "0.1.0",
         "private": True,
         "type": "module",
         "main": "micromegas_datafusion_wasm.js",
         "types": "micromegas_datafusion_wasm.d.ts",
     }
     ```
2. **`wasm-pack build`** (run manually, not documented): emits a richer `package.json` derived from
   `Cargo.toml` (`version` = workspace version `0.27.0`, no `private`, plus
   `collaborators`/`repository`/`license`/`keywords`/`files`/`sideEffects`/`homepage`, no trailing
   newline), structurally different `.js` glue, **and side artifacts** (`README.md` and a
   `.gitignore` containing `*`).

### What is tracked vs. ignored

- Root `.gitignore:72-75` keeps `.js`, `.d.ts`, `package.json` **tracked** but ignores the large
  `micromegas_datafusion_wasm_bg.wasm` (51 MB) and `micromegas_datafusion_wasm_bg.wasm.d.ts`.
- `git ls-files` confirms exactly three tracked files: `micromegas_datafusion_wasm.js`,
  `micromegas_datafusion_wasm.d.ts`, `package.json`.
- The `.js`/`.d.ts`/`package.json` are tracked deliberately so `yarn`/`tsc` resolve the package
  without a WASM toolchain (vite aliases `micromegas-datafusion-wasm` → this dir, `vite.config.ts:46`;
  consumed in `src/lib/wasm-engine.ts:6,10`). The `_bg.wasm` binary is **not** committed and must be
  built locally.
- The committed `package.json` is currently the **build.py form** (`0.1.0`/`private`).

### Evidence that `wasm-pack build` has been run into the output dir

`analytics-web-app/src/lib/datafusion-wasm/` contains untracked `README.md` and `.gitignore` (`*`).
`wasm-bindgen --target web` emits neither — only `wasm-pack` writes a `README.md` and a `*`
`.gitignore` into its output. These stray files are the fingerprint of a past `wasm-pack build`
into this directory and are the proximate source of the churn the issue describes.

### CI already treats build.py as de-facto canonical

`build/rust_ci.py:26` runs `python3 build.py --check` in the WASM pipeline. `check()`:
- rebuilds via `build()` (so `wasm-bindgen --target web` + the hardcoded `package.json`),
- normalizes Rust symbol hashes (`__hXXXX`) and wasm-bindgen glue hashes via
  `_normalize_symbol_hashes`,
- compares the three `TRACKED_BINDINGS` against `HEAD`.

So a commit containing **wasm-pack-form** bindings would fail CI (structural diff beyond hashes),
while build.py-form bindings pass. The wasm-pack divergence is therefore a **local developer
footgun** that produces noisy diffs and a red CI, not something that can land cleanly.

### Relevant commit history (decisions already made)

- `fb31297d1` — stopped tracking the generated files, added them to `.gitignore`.
- `50addbdee` — restored `package.json` because the yarn `file:` dependency / tsc need it.
- `7404ff0e4` — added `"private": true` specifically to **suppress a yarn workspace warning**.

The minimal `private` `package.json` is thus an intentional, load-bearing shape — not an accident.
The wasm-pack form (no `private`) regresses the yarn-warning fix.

## Design

### Decision: `build.py` is canonical (Option A)

`build.py` becomes the single supported generator. Rationale: CI already enforces it, the minimal
`private` `package.json` is a deliberate fix for a real yarn warning, and the vite alias + yarn
`file:` consumption never read the package version. `wasm-pack` is retained **only** for
`build.py --test` (headless Firefox integration tests), which runs `wasm-pack test` in the crate dir
(`build.py:80`, `cwd=CRATE_DIR`) and may populate `rust/datafusion-wasm/pkg/`, but never writes the
committed bindings in `OUTPUT_DIR`. Note this interacts with the copy loop: if a `--test` run leaves
a `README.md`/`.gitignore` in `pkg/`, the next `build()` copies them into `OUTPUT_DIR` (see step 2
hardening).

### `package.json` shape

Keep the minimal, private shape (one `version` field, no publish metadata). The open decision is
what `version` should hold:

- **Recommended — static placeholder, decoupled from the workspace.** Keep a fixed value and add a
  comment in `build.py` that this is a private, never-published, path-consumed package whose version
  is cosmetic. Zero release-time coupling: bumping the crate via `cargo release` never invalidates
  the committed bindings. (Switch the literal from the stale-looking `0.1.0` to something explicit,
  e.g. `0.0.0`, to signal "not a real version".)
- **Alternative — stamp the crate version from `Cargo.toml`.** Single source of truth, but every
  `cargo release -p micromegas-datafusion-wasm` (`build/release.py:44`) bumps `Cargo.toml` and would
  leave the committed `package.json` stale, so `build.py --check` would fail in CI until the bindings
  are regenerated. This requires wiring a regenerate+commit step into `release.py` (and a
  `wasm-bindgen` toolchain on the release machine). More moving parts for a value nothing reads.

This is the main **Open Question** for the user.

### Reproducibility

With the pinned toolchain (`rust/rust-toolchain.toml` → rustc 1.96.0; `build.py:check_tools()`
already asserts the installed `wasm-bindgen` CLI matches `Cargo.lock`) and a single generator, the
`.js`/`.d.ts`/`package.json` output is deterministic. The symbol-hash normalization in `check()` is
kept as defense-in-depth but should not be needed for byte-identity per the issue's analysis.

### Guarding against wasm-pack reintroducing divergence

We cannot stop a human from typing `wasm-pack build`, so the defense is layered:

1. **Documentation**: README states `build.py` is the *only* way to regenerate committed bindings and
   that `wasm-pack` is for `--test` only — never `wasm-pack build` into the output dir.
2. **Cleanup**: delete the stray `README.md` and `.gitignore` (`*`) already sitting in the output dir
   (they are untracked, so this is a working-tree cleanup, not a git change).
3. **CI** (already in place): `build.py --check` fails any commit whose bindings don't match
   build.py output, so divergence cannot land on `main` regardless.

Note the most likely real-world churn vector is not a manual `wasm-pack build` but the **supported**
`build.py --test` flow: it runs `wasm-pack test` in the crate dir and can itself seed `pkg/` with a
`README.md`/`.gitignore`, which the next `build()` then copies into `OUTPUT_DIR`. So `build()` should
sanitize what it copies out of `pkg/` (prune known wasm-pack leftovers from `OUTPUT_DIR`, per step 2)
regardless of how `pkg/` got populated — documentation alone does not cover this path.

### Flow (after change)

```
rust/datafusion-wasm/build.py            (the ONLY generator of committed bindings)
  └─ cargo build --target wasm32-unknown-unknown --release
  └─ wasm-bindgen --target web  --> pkg/{.js,.d.ts,_bg.wasm,_bg.wasm.d.ts}
  └─ [wasm-opt]
  └─ copy .js/.d.ts/_bg.wasm/_bg.wasm.d.ts -> analytics-web-app/src/lib/datafusion-wasm/
  └─ write package.json (single intentional shape)

wasm-pack ── used ONLY by `build.py --test` (headless Firefox), runs in the crate dir and may
             populate pkg/ ── never writes committed bindings in OUTPUT_DIR (but pkg/ leftovers can
             be copied into OUTPUT_DIR by the next build())
```

## Implementation Steps

1. **Decide the `version` policy** (Open Question). Default to the static-placeholder recommendation
   unless the user prefers stamping.
2. **`rust/datafusion-wasm/build.py`**:
   - Update `WASM_PACKAGE_JSON` to the agreed final shape (static placeholder version + a comment
     explaining it's a private path-consumed package), *or* implement Cargo.toml version stamping if
     the alternative is chosen.
   - (Optional hardening) In `build()`, after copying from `pkg/` into `OUTPUT_DIR`, prune known
     wasm-pack leftovers (`README.md`, `.gitignore`) **from `OUTPUT_DIR`** — not just from `pkg/`.
     The copy loop (`build.py:114-119`) is additive and never deletes pre-existing files in
     `OUTPUT_DIR`, so pruning `pkg/` alone would leave already-copied strays in place and the loop
     would re-propagate any such file from `pkg/` on every build. Targeting `OUTPUT_DIR` (or making
     `OUTPUT_DIR` an exact mirror of the intended file set) makes the step self-healing. Keep this
     conservative — only remove known wasm-pack leftovers, not developer files.
3. **Regenerate and commit the canonical bindings** so the committed form is unambiguously the
   build.py form: run `python3 rust/datafusion-wasm/build.py` and commit the resulting
   `.js`/`.d.ts`/`package.json` if they changed.
4. **Clean up stray artifacts**: remove the untracked `README.md` and `.gitignore` (`*`) from
   `analytics-web-app/src/lib/datafusion-wasm/`.
5. **`rust/datafusion-wasm/README.md`**: rewrite the build section to state build.py is canonical;
   reframe "Manual Build" as a debugging aid that produces the *same* `wasm-bindgen --target web`
   output (and note it does **not** write `package.json`); add an explicit "do not run
   `wasm-pack build` into the output dir — `wasm-pack` is for tests only" note. Also flag that even
   the supported `build.py --test` flow can seed `pkg/` with wasm-pack strays, so `build()` must
   sanitize what it copies (step 2 hardening) rather than relying on documentation alone.
6. **Verify reproducibility**: from a clean working tree, run `build.py`, confirm `git diff` is empty
   (or only expected hash churn), then run `build.py --check` and confirm it passes.

## Files to Modify

- `rust/datafusion-wasm/build.py` — finalize `package.json` shape; optional output-pruning.
- `rust/datafusion-wasm/README.md` — document build.py as canonical; forbid `wasm-pack build`.
- `analytics-web-app/src/lib/datafusion-wasm/package.json` — regenerated (tracked).
- `analytics-web-app/src/lib/datafusion-wasm/micromegas_datafusion_wasm.js` — regenerated if it
  currently reflects a wasm-pack run (tracked).
- `analytics-web-app/src/lib/datafusion-wasm/micromegas_datafusion_wasm.d.ts` — regenerated if needed
  (tracked).
- `analytics-web-app/src/lib/datafusion-wasm/README.md`, `.gitignore` — **delete** (untracked stray
  wasm-pack artifacts).

No changes expected to root `.gitignore` (the tracked/ignored split is correct) or to
`build/rust_ci.py` (the `--check` step already enforces canonicality).

## Trade-offs

- **Option A (build.py canonical) vs Option B (wasm-pack canonical).** Chose A: it matches the
  existing CI check, preserves the deliberate `private` package.json (yarn-warning fix from
  `7404ff0e4`), and avoids importing wasm-pack's publish-oriented metadata into a package that is
  only ever consumed by path. Option B would mean retiring build.py's hand-written package.json,
  reintroducing the yarn workspace warning (no `private`), and rewriting the CI check — more churn
  for no consumer benefit.
- **Static version vs Cargo.toml-stamped version.** Static avoids release-time CI breakage and a
  toolchain requirement on the release machine, at the cost of a version field that doesn't track the
  crate. Since the package is private and path-consumed, nothing reads the version, so the
  maintenance cost of stamping outweighs its (cosmetic) benefit. Documented as an Open Question in
  case the user wants the single-source-of-truth property anyway.
- **Keeping symbol-hash normalization in `check()`.** Could be dropped if output is truly
  byte-deterministic under the pinned toolchain, but it's cheap insurance against future toolchain
  nuances; left in place.

## Documentation

- `rust/datafusion-wasm/README.md` — primary update (build path, wasm-pack restriction).
- No mkdocs site pages cover this internal build detail; none need updating. If a contributor/build
  doc exists that mentions WASM bindings, mirror the "build.py only" guidance there (none found in
  this pass).

## Testing Strategy

- **Reproducibility**: on a clean checkout with the pinned toolchain, run
  `python3 rust/datafusion-wasm/build.py`; `git diff` on the three tracked files should be empty
  (modulo normalized hashes). Repeat to confirm idempotence.
- **Freshness check**: `python3 rust/datafusion-wasm/build.py --check` passes (this is what CI runs,
  `build/rust_ci.py:26`).
- **No stray artifacts**: after a build, the output dir contains only the intended files; no
  `README.md`/`.gitignore` reappear from `pkg/`.
- **Web app still resolves the package**: `cd analytics-web-app && yarn install && yarn type-check &&
  yarn build` succeed (validates the package.json shape is acceptable to yarn/tsc and that the
  `private` field still suppresses the workspace warning).
- **WASM integration tests** unaffected: `python3 rust/datafusion-wasm/build.py --test` still runs.

## Open Questions

1. **`package.json` version**: static placeholder decoupled from the workspace (recommended, zero
   release coupling) vs. stamp the crate version from `Cargo.toml` (single source of truth, but
   requires regenerating bindings on every wasm-crate release and a toolchain on the release box)?
2. **`build.py` output-pruning** (step 2, optional): do we want build.py to actively delete known
   wasm-pack leftovers from `OUTPUT_DIR` after the copy (the strays live there, not in `pkg/`, and
   the additive copy loop re-propagates any present in `pkg/`), or is documentation + the existing
   CI check sufficient?
