# Cheaper Dynamic Property Path for Log and Metric Calls Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1682

## Overview

A log or metric call that has a runtime property value costs about 4× a call with an interned
set (≈95–118 ns against 25 ns single-threaded), even when the value has low cardinality and the
block already holds the resulting set. Every `imetric_dyn!`/`fmetric_dyn!` call, the C API
`*_tagged` calls and the interop log bridges take this path too. This plan removes the per-call
heap allocation, sort and nested scan from `merge_properties`, and moves the content hash out of
the stream lock. If the profile shows hashing still matters after that, it also precomputes the
hashes of static entries. Nothing changes in the wire format, the SQL surface or the sets a block
ends up holding.

## Current State

All four dynamic dispatch paths in `rust/tracing/src/dispatch.rs` follow the same pattern:
`merge_properties` runs before the lock, and `intern_set` runs under it.

- `tagged_float_metric_dyn` (`:863`–`:866`) and `tagged_integer_metric_dyn` (`:888`–`:891`)
- `log_dyn_tagged_to_sink` (`:1071`, lock at `:1079`, `intern_set` at `:1081`). It also passes
  `&merged.properties` to `sink.on_log`.
- `log_interop_dyn_tagged` (`:1106`, `intern_set` at `:1126`). It merges with
  `PropertySet::empty()` and iterates `merged.properties` to filter `target`/`level` before
  calling the sink.

`merge_properties` (`rust/tracing/src/dyn_deps.rs:69`):

1. `static_set.dyn_properties().to_vec()` allocates a heap `Vec` on every call.
2. For each dynamic property, it runs `retain` over the whole list with a case-insensitive name
   compare, then pushes.
3. It runs `sort_by(cmp_dyn_properties)` (descending byte order of the name) over the whole list.
4. It builds the `static_values` mask with a nested scan. For each merged entry it looks for a
   static property with the same value pointer, length and name. `static_names` comes from the
   per-entry `is_static_name` flag.

`DynDeps::intern_set` (`dyn_deps.rs:334`), under the stream lock:

1. `content_hash` (`:104`) runs two chained `xxh3_64_with_seed` calls per property, one on the
   name and one on the value.
2. It probes `sets: HashIndex<DynSet>` and compares each candidate with `same_content`, which
   reads names and values back out of the queue bytes.
3. On a miss, it interns the strings and pushes a set record. A hit allocates nothing.

Invariants to keep:

- **Content identity.** The hash and `same_content` depend only on the (name, value) content.
  So a set split between static and dynamic properties dedupes with the same set given all
  dynamic (`split_between_static_and_dynamic_does_not_change_identity`,
  `rust/tracing/tests/dyn_deps_tests.rs:90`).
- **Override semantics.** A dynamic property replaces every static property whose name matches
  case-insensitively. Among dynamic properties, the last of a case-insensitive name wins
  (`merge_dynamic_overrides_static_case_insensitively`, `:322`). The macros and the C API depend
  on "last wins" (`macros.rs:984`, `capi/src/lib.rs:74`).
- **Canonical order.** The order is `cmp_dyn_properties`, the same as `sort_properties`.
  `sort_by` is stable, so case-sensitive duplicates inside a static set keep their relative
  order. That can only happen in a set built directly with `PropertySet::find_or_create`.
- **Masks are conservative.** A bit is set only when the string really is `'static`. A cleared
  bit is always safe, because the value then gets a string record. Entries past index 63 are
  never marked.

Benchmarks: `rust/tracing/benches/properties.rs` has `log/dyn_low_card`, `log/dyn_high_card`,
`log/dyn_mixed`, `metric/dyn_low_card`, `metric/dyn_high_card` and the `contention` group.
`benches/with_dispatch.rs` has `dispatch/metric_dyn`, `metric_dyn_high_card` and
`metric_dyn_prop`.

## Design

### 1. Profile first

Save criterion baselines and take a `perf` profile of `metric/dyn_low_card` and
`log/dyn_mixed`, so we know whether allocation, sort or hashing dominates (see Implementation
Steps). Steps 2 and 3 are worth doing whatever the profile shows. Step 4 is gated on it.

### 2. `merge_properties`: stack buffer and a merge of two sorted lists

`MergedProperties` changes to:

```rust
pub struct MergedProperties<'a> {
    properties: SmallVec<[DynProperty<'a>; INLINE_PROPERTIES]>,
    static_values: u64,
    static_names: u64,
    hash: u64, // step 3
}

impl<'a> MergedProperties<'a> {
    pub fn properties(&self) -> &[DynProperty<'a>] { &self.properties }
}
```

`INLINE_PROPERTIES = 8`. A `DynProperty` is 40 bytes, so the inline buffer is 320 bytes of stack.
That covers the benches and typical callsites, and longer lists spill to the heap without a cap.
The field becomes private behind a slice accessor so the storage type can change without
touching callers.

The algorithm uses the fact that the static set's `dyn_view` is already in canonical order:

1. **Dedupe the dynamic properties** into a second small stack buffer, `SmallVec<[_; 8]>`.
   For each property, remove any earlier entry whose name matches case-insensitively, then push
   it (last wins). Sort this short list with `cmp_dyn_properties`. It is usually one to three
   entries.
2. **Merge-walk** the static `dyn_view` and the sorted dynamic list into `properties`:
   - Skip a static entry if any deduped dynamic name matches it case-insensitively. This is the
     same O(|static|·|dyn|) compare as today's `retain`, without the moves.
   - Otherwise emit whichever head comes first under `cmp_dyn_properties`. A static and a
     dynamic head can never tie, since an exact name match was skipped.
   - Static entries are emitted in their original order, so the result matches today's stable
     sort exactly, case-variant duplicates included.
3. **Masks are set on push.** An entry emitted from the static side sets bit `i` of
   `static_values` (when `i < 64`). Every entry ORs `is_static_name()` into `static_names`. The
   nested pointer scan goes away.

Behaviour change: before, a dynamic property whose value pointed at the exact memory of a
static set's value was marked static. Now only entries that come from the static set are. That
case only arises if a caller builds a `DynProperty` from `set.dyn_properties()` by hand. The
result is still correct, since the string gets a record, so the change is safe.

### 3. Hash outside the lock

`merge_properties` computes `hash` as the last step, folding over `properties` in output order
with the current `content_hash` function. `intern_set` uses `merged.hash` instead of computing
it. Only the probe, the `same_content` compare and any miss work stay under the lock.
`content_hash` keeps its definition, so content identity is unaffected.

### 4. Precomputed static entry hashes (only if the profile still shows hashing)

The hash is in-process only, so its definition can change freely. Change it to fold per-entry
hashes:

```
entry_hash(name, value) = xxh3_64_with_seed(value, xxh3_64(name))
set_hash = fold(0, |h, e| (h.rotate_left(5) ^ e).wrapping_mul(K))
```

`PropertySet` gains `entry_hashes: Box<[u64]>` alongside `dyn_view`, computed in
`find_or_create` and excluded from `Eq`/`Hash` like `dyn_view`. The merge walk then takes a
static entry's hash from the slice, so the only xxh3 work is for dynamic entries. A static entry
and a dynamic entry with the same content still hash the same, so content identity holds.
Hashing static strings by pointer, the issue's other idea, is rejected (see Trade-offs).

## Implementation Steps

1. **Baseline.** From `rust/`:
   `cargo bench -p micromegas-tracing --bench properties -- --save-baseline before` and
   `cargo bench -p micromegas-tracing --bench with_dispatch -- --save-baseline before`.
   Profile with `perf record -g` on the `properties` bench binary filtered to
   `metric/dyn_low_card` and `log/dyn_mixed`. Write the split (allocation, sort, mask scan,
   hash, probe) into the PR description.
2. **Dependency.** Add `smallvec = "1.15"` to `[workspace.dependencies]` in `rust/Cargo.toml`
   (it is already in `Cargo.lock` transitively, MIT/Apache-2.0). Add `smallvec.workspace = true`
   to the native-only section of `rust/tracing/Cargo.toml`, since `dyn_deps` is native-only.
3. **`merge_properties`.** Rewrite it in `rust/tracing/src/dyn_deps.rs` as in Design §2. Make
   `properties` private and add `MergedProperties::properties()`.
4. **Callers.** Update `rust/tracing/src/dispatch.rs` (`:1073` `sink.on_log`, `:1111`/`:1112`
   interop filter) and the tests that read `merged.properties`:
   `rust/tracing/tests/dyn_deps_tests.rs`, `rust/analytics/tests/log_tests.rs`,
   `rust/analytics/tests/metrics_test.rs` and `rust/capi/tests/properties_tests.rs`.
5. **Hash outside the lock.** Add `hash` to `MergedProperties`, compute it in
   `merge_properties`, and use it in `intern_set`.
6. **Measure.** Rerun both benches with `--baseline before`. If the hash is still a visible
   share of `metric/dyn_low_card` or `log/dyn_mixed` in a fresh profile, do step 7. Otherwise
   skip it and record that under Decisions.
7. **(Gated) Static entry hashes.** Implement Design §4 in `rust/tracing/src/property_set.rs`
   (`PropertySet::entry_hashes`, filled in `find_or_create`) and `dyn_deps.rs` (`entry_hash`,
   new fold, merge walk reads precomputed hashes).
8. **CHANGELOG.** Add an Unreleased entry with before/after medians and a **Minor breaking
   change** clause: `MergedProperties::properties` is now a method returning a slice.

## Files to Modify

- `rust/Cargo.toml`: workspace `smallvec`
- `rust/tracing/Cargo.toml`: `smallvec` (native-only)
- `rust/tracing/src/dyn_deps.rs`: `MergedProperties`, `merge_properties`, `intern_set`,
  maybe `content_hash`
- `rust/tracing/src/dispatch.rs`: accessor use
- `rust/tracing/src/property_set.rs`: only for step 7
- `rust/tracing/tests/dyn_deps_tests.rs`, `rust/analytics/tests/log_tests.rs`,
  `rust/analytics/tests/metrics_test.rs`, `rust/capi/tests/properties_tests.rs`: accessor and
  new tests
- `CHANGELOG.md`

## Trade-offs

- **Merge walk vs. "insert dynamic entries into the copied static list".** Inserting into a
  copy of the static list means shifting the static-value mask on every removal and insertion.
  A walk over two sorted inputs knows each entry's origin when it emits it, so the masks are set
  as it pushes and nothing needs shifting.
- **`SmallVec` vs. a fixed array with a fallback, or a thread-local scratch `Vec`.** `SmallVec`
  gives the stack common case and the uncapped heap fallback the issue asks for, in a crate
  already in the lock file. A thread-local scratch buffer cannot be returned in
  `MergedProperties` without borrowing problems, and it costs a TLS access on every call.
- **Hashing static strings by pointer: rejected.** A split set and the same set given all
  dynamic would hash differently. That breaks content identity
  (`split_between_static_and_dynamic_does_not_change_identity`) and would write duplicate set
  records. Precomputed content hashes (§4) save the same work and keep identity.
- **Hash stored in `MergedProperties` vs. passed as an `intern_set` argument.** Storing it
  means a caller cannot pair a hash with the wrong list, and every caller gets it for free.

## Decisions

## Testing Strategy

Unit tests in `rust/tracing/tests/dyn_deps_tests.rs` (no DB):

- The existing tests must pass unchanged apart from the accessor, especially
  `split_between_static_and_dynamic_does_not_change_identity`,
  `merge_dynamic_overrides_static_case_insensitively` and `static_values_get_no_string_record`.
- **Reference equivalence:** keep a copy of today's `Vec`/`retain`/`sort` algorithm in the test
  file as a reference. Over a table of cases, assert that the new `merge_properties` gives the
  same `(name, value)` sequence, `is_static_value`/`is_static_name` per index, and hash. Cases:
  - empty static set or empty dynamic list
  - a dynamic name that sorts before, between and after the static names
  - a case-insensitive override of a static entry
  - case-insensitive duplicates within the dynamic list (last wins)
  - a static set built directly with case-variant duplicates
  - `with_dyn_name` entries
  - more than 8 entries (heap spill)
  - more than 64 entries (mask bits beyond 63 stay clear)
  The masks need `pub(crate)` test access or a `#[doc(hidden)] pub` getter, following the
  existing test seams in the crate.
- If step 7 lands, the hash of a set built split and all-dynamic is equal, and different
  content gives different hashes on the existing cases.

Performance is checked by the criterion runs in Implementation Steps 1 and 6. There is no
automated performance gate, which matches the precedent plans.

## Documentation

No `mkdocs/` page describes these internals. The CHANGELOG entry is enough.
