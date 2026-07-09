# Rename `MICROMEGAS_L1_CACHE_MB` → `MICROMEGAS_OBJECT_CACHE_L1_MB` Plan

## Overview
Rename the in-process L1 cache sizing env var from `MICROMEGAS_L1_CACHE_MB` to
`MICROMEGAS_OBJECT_CACHE_L1_MB`, bringing it into the existing `MICROMEGAS_OBJECT_CACHE_*`
family. The in-process L1 cache and the `object-cache-srv` are the *same* object-cache
subsystem (both sit on the `object-cache` crate's `RangeCache`; they differ only in backend —
`BoundedMemoryBackend` RAM-only for L1, `FoyerBackend` RAM+disk for the srv). Naming on a
standalone `L1_` prefix hid that relationship and, more importantly, "L1" alone won't uniquely
identify the knob once the planned DataFusion metadata cache lands as a second in-process cache.
The new name keeps the `OBJECT_CACHE` family prefix contiguous and uses `L1` only as the *tier*
qualifier within it, distinguishing it from the server's own RAM tier (`MICROMEGAS_OBJECT_CACHE_RAM_MB`).

## Current State
The var is defined once and read once, in `rust/object-cache/src/l1_store.rs`:

- Line 22: `const ENV_L1_CACHE_MB: &str = "MICROMEGAS_L1_CACHE_MB";`
- Line 24: `const DEFAULT_L1_CACHE_MB: u64 = 200;` (default budget; `0` disables L1)
- Line 47: read via `std::env::var(ENV_L1_CACHE_MB)` in `shared_l1_backend()`
- Lines 41, 50, 57, 73: the const is interpolated into the doc comment and the
  invalid-value / disabled log messages

Documentation and other string references to the literal `MICROMEGAS_L1_CACHE_MB`:

- `mkdocs/docs/admin/object-cache.md:192-194` — the "In-process L1 cache" section. Note line
  193 currently frames the knob as *"unrelated to `MICROMEGAS_OBJECT_CACHE_RAM_MB`"*; after the
  rename the two are siblings in one family (different tiers), so the prose needs adjusting, not
  just the identifier.
- `CHANGELOG.md:24` — the entry sits under `## Unreleased`, so the knob has no released
  consumers. Update the string in place; **no** deprecation alias or migration note is warranted.
- `tasks/completed/l1_inprocess_cache_plan.md` (lines 121, 234, 240, 291, 374) — a completed
  historical plan.

Behavior is unchanged: this is a string/identifier rename only, no logic, defaults, or wiring
touched.

## Design
Pure rename. Two facets:

1. **Env var string** (the operator-facing contract): `"MICROMEGAS_L1_CACHE_MB"` →
   `"MICROMEGAS_OBJECT_CACHE_L1_MB"`. This is the only functionally meaningful change.
2. **Rust const identifiers** (internal, for readability/consistency): rename
   `ENV_L1_CACHE_MB` → `ENV_OBJECT_CACHE_L1_MB` and `DEFAULT_L1_CACHE_MB` →
   `DEFAULT_OBJECT_CACHE_L1_MB` so the code reads consistently with the new var. These are
   `const`s private to `l1_store.rs`; renaming them is self-contained.

The `MB` suffix continues to read as the RAM/memory budget. If L1 later grows a local disk tier
(the `BoundedMemoryBackend` → foyer hybrid path), it gets a sibling `MICROMEGAS_OBJECT_CACHE_L1_DISK_GB`,
mirroring the server's `_RAM_MB` + `_DISK_GB` pair — out of scope here, noted for direction only.

## Implementation Steps
1. In `rust/object-cache/src/l1_store.rs`:
   - Change the string literal on line 22 to `"MICROMEGAS_OBJECT_CACHE_L1_MB"`.
   - Rename const `ENV_L1_CACHE_MB` → `ENV_OBJECT_CACHE_L1_MB` and update its two references
     (env read on line 47, log messages on lines 50 and 57).
   - Rename const `DEFAULT_L1_CACHE_MB` → `DEFAULT_OBJECT_CACHE_L1_MB` and its references.
   - Update the doc-comment mentions of the literal `MICROMEGAS_L1_CACHE_MB` (lines 20-21, 41, 73)
     to the new name.
2. In `mkdocs/docs/admin/object-cache.md` (lines 192-194): replace the env var name, and reword
   the "unrelated to `MICROMEGAS_OBJECT_CACHE_RAM_MB`" sentence to reflect that both now belong to
   the `MICROMEGAS_OBJECT_CACHE_*` family — `_L1_MB` sizes the in-process L1 tier, `_RAM_MB` sizes
   this server's RAM tier.
3. In `CHANGELOG.md` (line 24, under `## Unreleased`): update the env var string in place.
4. Run `cargo fmt` and `cargo clippy --workspace -- -D warnings` from `rust/`.
5. Verify no stray references remain (see Testing Strategy).

## Files to Modify
- `rust/object-cache/src/l1_store.rs`
- `mkdocs/docs/admin/object-cache.md`
- `CHANGELOG.md`

## Trade-offs
- **Chosen: `MICROMEGAS_OBJECT_CACHE_L1_MB`.** Keeps the `OBJECT_CACHE` family prefix contiguous
  (reads alongside `_URL`, `_RAM_MB`, `_DISK_GB`), reflecting that L1 and the srv are one
  subsystem in two deployment modes; `L1` is the tier qualifier. Leaves the future metadata cache
  free to be named on its own subsystem axis (`MICROMEGAS_METADATA_CACHE_MB`) rather than sharing
  an `L1_` grouping with an unrelated cache.
- **Rejected: keep `MICROMEGAS_L1_CACHE_MB`.** "L1" names only the tier; once the metadata cache
  is a second in-process (L1-tier) cache, a bare `L1_CACHE_MB` is ambiguous about *which* L1.
- **Rejected: `MICROMEGAS_OBJECT_CACHE_MB` (bare) / `MICROMEGAS_RAM_OBJECT_CACHE_MB`.** Both
  coexist with the srv's `MICROMEGAS_OBJECT_CACHE_RAM_MB` in tiered deployments and must be sized
  independently; a bare `_MB` next to `_RAM_MB` is ambiguous, and `RAM_OBJECT_CACHE_MB` is a
  confusing word-order near-anagram of the existing `OBJECT_CACHE_RAM_MB`.
- **Rejected: `MICROMEGAS_L1_OBJECT_CACHE_MB`.** Splits the established `OBJECT_CACHE` prefix,
  reading as a different family rather than a tier within the existing one.

## Documentation
- `mkdocs/docs/admin/object-cache.md` — "In-process L1 cache" section (rename + reframe the
  relationship to `_RAM_MB`, per step 2).
- `CHANGELOG.md` — update the Unreleased entry string.

## Testing Strategy
- `grep -rn "MICROMEGAS_L1_CACHE_MB\|ENV_L1_CACHE_MB\|DEFAULT_L1_CACHE_MB" .` returns nothing
  (outside the historical `tasks/completed/` plan, if intentionally left as-is).
- `cargo build` and `cargo clippy --workspace -- -D warnings` succeed from `rust/`.
- Manual sanity: set `MICROMEGAS_OBJECT_CACHE_L1_MB=0` and confirm the "in-process L1 cache
  disabled" log line fires; set a positive value and confirm the "budget=NMB" enable log.

## Open Questions
- **`tasks/completed/l1_inprocess_cache_plan.md`**: leave the old env var name in that completed
  plan as a historical record (recommended — it documents the state at the time), or update it for
  consistency? Default: leave it.
- **Const identifier renames**: rename `ENV_L1_CACHE_MB`/`DEFAULT_L1_CACHE_MB` for consistency
  (recommended), or change only the string literal and keep the shorter const names? Default:
  rename both.
