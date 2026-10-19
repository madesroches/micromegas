# Cheaper Dynamic Property Path for Log and Metric Calls Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1682

## Overview

A log or metric call that has a runtime property value costs about 4× a call with an interned
set (≈95–118 ns against 25 ns single-threaded), even when the value has low cardinality and the
block already holds the resulting set. Every `imetric_dyn!`/`fmetric_dyn!` call, the C API
`*_tagged` calls and the interop log bridges take this path too. This plan removes the per-call
heap allocation, sort and nested scan from `merge_properties`. If the profile shows the content
hash under the stream lock matters, it also moves that hash out of the lock, and if hashing still
matters after that, it precomputes the hashes of static entries. Nothing changes in the wire format, the SQL surface or the sets a block
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
Steps). §2 is worth doing whatever the profile shows. §3 and §4 are gated on it.

### 2. `merge_properties`: stack buffer and a merge of two sorted lists

`MergedProperties` changes to:

```rust
pub struct MergedProperties<'a> {
    properties: SmallVec<[DynProperty<'a>; INLINE_PROPERTIES]>,
    static_values: u64,
    static_names: u64,
    hash: u64, // only with §3
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
static set's value was marked static. Now only entries that come from the static set are. The
same case can arise through the `..set` spread macro path, where the compiler may pool a
callsite literal with an identical value in the spread set (e.g. `log!(region = "eu", ..set)`
where the set has `("region", "eu")`), or when a caller builds a `DynProperty` from
`set.dyn_properties()` by hand. The only cost is one extra string record.

### 3. Hash outside the lock (only if the profile shows lock-held hashing, notably in the `contention` group)

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
Hashing static strings by pointer, the issue's other idea, is rejected (see Decisions).

## Implementation Steps

1. **Baseline.** From `rust/`:
   `cargo bench -p micromegas-tracing --bench properties -- --save-baseline before` and
   `cargo bench -p micromegas-tracing --bench with_dispatch -- --save-baseline before`.
   Profile with `perf record -g` on the `properties` bench binary filtered to
   `metric/dyn_low_card` and `log/dyn_mixed`. Write the split (allocation, sort, mask scan,
   hash, probe) into the PR description. Also run the otel-comparison bench on the same
   machine (`cargo bench -p micromegas-otel-comparison-bench | tee bench-before.log`) so step 8
   has a like-for-like before.
2. **Dependency.** Add `smallvec = "1.15"` to `[workspace.dependencies]` in `rust/Cargo.toml`
   (MIT/Apache-2.0). It is already in the workspace `Cargo.lock` transitively, but becomes a new direct dependency of the published `micromegas-tracing` crate. Add `smallvec.workspace = true`
   to the native-only section of `rust/tracing/Cargo.toml`, since `dyn_deps` is native-only.
3. **`merge_properties`.** Rewrite it in `rust/tracing/src/dyn_deps.rs` as in Design §2. Make
   `properties` private and add `MergedProperties::properties()`. Add `#[doc(hidden)] pub`
   getters `is_static_value(i)` and `is_static_name(i)` on `MergedProperties`, and make
   `content_hash`, `DynProperty::is_static_name` and `cmp_dyn_properties` `#[doc(hidden)] pub`
   for the reference test.
4. **Callers.** Update `rust/tracing/src/dispatch.rs` (`:1073` `sink.on_log`, `:1111`/`:1112`
   interop filter) and the tests that read `merged.properties`:
   `rust/tracing/tests/dyn_deps_tests.rs` and `rust/capi/tests/properties_tests.rs`.
5. **(Gated) Hash outside the lock.** Only if the step 1 profile justifies Design §3. Add `hash` to `MergedProperties` with a
   `#[doc(hidden)] pub` `hash()` getter, compute it in `merge_properties`, use it in
   `intern_set`, and add the hash comparison to the reference-equivalence test.
6. **Measure.** Rerun both benches (`properties`, `with_dispatch`) with
   `--baseline before`. If the hash is still a visible share of `metric/dyn_low_card` or
   `log/dyn_mixed` in a fresh profile, do step 7. Otherwise skip it and record that under
   Decisions.
7. **(Gated) Static entry hashes.** Implement Design §4 in `rust/tracing/src/property_set.rs`
   (`PropertySet::entry_hashes`, filled in `find_or_create`) and `dyn_deps.rs` (`entry_hash`,
   new fold, merge walk reads precomputed hashes).
8. **Published figures.** Rerun the full otel-comparison bench, following the Reproduce section of
   `mkdocs/docs/when-to-use/instrumentation-overhead.md`
   (`cargo bench -p micromegas-otel-comparison-bench | tee bench.log`, then
   `python3 otel-comparison-bench/report.py bench.log`). Regenerate every table in that page and
   update its hardware/software/date table. Put the `dyn_*` before/after medians from
   `bench-before.log` and `bench.log` in the PR description.
9. **CHANGELOG.** Add an Unreleased entry with before/after medians and a **Minor breaking
   change** clause: `MergedProperties::properties` is now a method returning a slice.

## Files to Modify

- `rust/Cargo.toml`: workspace `smallvec`
- `rust/tracing/Cargo.toml`: `smallvec` (native-only)
- `rust/tracing/src/dyn_deps.rs`: `MergedProperties` (plus `#[doc(hidden)] pub` getters),
  `merge_properties`, `intern_set`, `content_hash` (`#[doc(hidden)] pub`)
- `rust/tracing/src/dispatch.rs`: accessor use
- `rust/tracing/src/property_set.rs`: `DynProperty::is_static_name` and `cmp_dyn_properties`
  become `#[doc(hidden)] pub`; `entry_hashes` only for step 7
- `rust/tracing/tests/dyn_deps_tests.rs`, `rust/capi/tests/properties_tests.rs`: accessor and
  new tests
- `mkdocs/docs/when-to-use/instrumentation-overhead.md`: all tables regenerated, hardware/software/date table updated
- `CHANGELOG.md`

## Decisions

- Hashing before the lock was tried and reverted earlier with no measurable difference, so
  Design §3 is gated on the profile.
- Hashing static strings by pointer is rejected: it breaks content identity for split sets
  (`split_between_static_and_dynamic_does_not_change_identity`).

- **Step 5 (hash outside the lock): not adopted.** `perf` is not installed on this machine, so
  there is no profile; the gate was decided from criterion. I implemented it, and
  `contention/log/dyn_high_card/8` stayed at 6.4 us before, 6.4 us after the merge rewrite alone
  and 6.8 us with the hash moved out (4 threads: 3.13, 3.31 and 2.96 us), and
  `contention/metric/dyn_high_card/8` went 4.1, 4.2 and 4.6 us. No difference beyond noise, so the
  change was reverted. This repeats the earlier result.
- **Step 7 (precomputed static entry hashes): not adopted.** To bound the share of hashing, I
  temporarily replaced `content_hash` with a constant-cost function (the property count):
  `metric/dyn_low_card` measured 53.2 ns against 50.1 ns with the real hash, and `log/dyn_low_card`
  60.5 ns against 55.1 ns. Removing the hash entirely does not make the path faster, so
  precomputing part of it cannot pay off. The remaining ~30 ns over `static_low_card` is the lock,
  the probe and `same_content`. The later profile (see Results) qualifies this: the
  property-count hash makes every set of a given size collide, which moves the saved cost into
  the probe, and the real hash is 16% of `metric/dyn_low_card` and 9% of `log/dyn_mixed`. Step 7
  would still save nothing on `metric/dyn_low_card`, whose only property is dynamic, and at most
  that 9% on `log/dyn_mixed`, where three of four entries are static; it stays out of this change.
- **Merge fast path for an empty static set.** The `_dyn` metric macros and interop logs merge
  into an empty static set. With the plain merge-walk, `dispatch/metric_dyn` regressed from 68.9
  to 74.1 ns, because two copies of the 320-byte inline buffer cost more than the `Vec` they
  replaced. Deduping in the output buffer and returning it as-is when the static set is empty
  brought it to 65 ns.

## Testing Strategy

Unit tests in `rust/tracing/tests/dyn_deps_tests.rs` (no DB):

- The existing tests must pass unchanged apart from the accessor, especially
  `split_between_static_and_dynamic_does_not_change_identity`,
  `merge_dynamic_overrides_static_case_insensitively` and `static_values_get_no_string_record`.
- **Reference equivalence:** keep a copy of today's `Vec`/`retain`/`sort` algorithm in the test
  file as a reference. Over a table of cases, assert that the new `merge_properties` gives the
  same `(name, value)` sequence and `is_static_value`/`is_static_name` per index (and the hash, if step 5 lands). Override values in the table must differ from the static values they replace, so the reference's pointer-based mask agrees with the new code. Cases:
  - empty static set or empty dynamic list
  - a dynamic name that sorts before, between and after the static names
  - a case-insensitive override of a static entry
  - case-insensitive duplicates within the dynamic list (last wins)
  - a static set built directly with case-variant duplicates
  - `with_dyn_name` entries
  - more than 8 entries (heap spill)
  - more than 64 entries (mask bits beyond 63 stay clear)
  The test crate reaches the internals through `#[doc(hidden)] pub` getters on
  `MergedProperties` (`is_static_value(i)`, `is_static_name(i)`, plus `hash()` with step 5). It builds the
  reference copy using `DynProperty::is_static_name`, `cmp_dyn_properties` and `content_hash`,
  which also become `#[doc(hidden)] pub`.
- If step 7 lands, the hash of a set built split and all-dynamic is equal, and different
  content gives different hashes on the existing cases.

Performance is checked by the criterion runs in Implementation Steps 1 and 6. Acceptance: no
regression beyond noise on any tracing bench case that is run. There is no automated performance gate, which
matches the precedent plans.

## Documentation

No `mkdocs/` page describes these internals, but `instrumentation-overhead.md` publishes
per-call medians, which are regenerated in step 8 along with the rest of the page. Add the CHANGELOG entry.

## Results

`perf` was not available while steps 1 to 7 ran, so the gates for steps 5 and 7 were decided
from criterion (see Decisions). It was installed afterwards and both cases were profiled on the
old and new code (`perf record -e cpu-clock --call-graph dwarf`, 8 s `--profile-time` runs,
samples bucketed by the first matching frame from the leaf):

| bucket | `metric/dyn_low_card` before | after | `log/dyn_mixed` before | after |
|---|---|---|---|---|
| allocation | 15.7% | 0.7% | 26.2% | 12.1% |
| sort | 0.0% | 0.4% | 5.0% | 0.8% |
| mask scan / merge walk | 12.0% | 19.9% | 9.6% | 18.2% |
| content hash | 16.8% | 15.9% | 10.0% | 8.6% |
| probe + `same_content` | 24.0% | 25.1% | 18.6% | 22.7% |
| `now()` | 13.9% | 11.4% | 3.5% | 4.5% |
| lock + event push | 7.4% | 9.1% | 11.7% | 12.8% |
| other (dispatch, bench loop) | 10.1% | 17.4% | 15.2% | 20.3% |

Allocation was the largest removable cost and is gone from the metric path; the `log/dyn_mixed`
allocation left is its formatted message. The merge walk's share includes copying the inline
buffers by value. Hashing is 9 to 16% of the path, all of it single-threaded work outside any
contention.

Medians are criterion's central estimate on the machine described in
`instrumentation-overhead.md`, single-threaded unless noted. `before` is the `main` merge, `after`
is this change.

`properties` bench:

| case | before (ns) | after (ns) |
|---|---|---|
| `log/dyn_low_card` | 56.5 | 50.0 |
| `log/dyn_high_card` | 202.7 | 198.5 |
| `log/dyn_mixed` | 266.4 | 224.2 (212 in repeat runs) |
| `metric/literals_set` | 124.5 | 98.6 |
| `metric/dyn_low_card` | 51.1 | 46.9 |
| `metric/dyn_high_card` | 156.6 | 145.2 |
| `contention/log/dyn_high_card/4` | 3131 | 3020 |
| `contention/log/dyn_high_card/8` | 6539 | 7090 (noise: 6.4 to 6.8 us in other runs) |

`with_dispatch` bench:

| case | before (ns) | after (ns) |
|---|---|---|
| `dispatch/metric_dyn_prop` | 45.7 | 43.8 |
| `dispatch/metric_dyn` | 68.9 | 66.0 |
| `dispatch/metric_dyn_high_card` | 133.4 | 144.1 (140 to 147 in repeat runs) |
| `dispatch/log_interop` | 55.2 | 58.9 (56 to 58 in repeat runs) |

Two cases are slightly slower and I could not make them match the old code: the high-cardinality
metric and the interop log, both by a few percent. They are dominated by interning new strings and
by the sink call, lock and `args.to_string()` inside `Dispatch::log_interop`.

otel-comparison bench (criterion `time:` estimates, `bench-before.log` against `bench.log`, full
runs; the standalone re-runs of the two high-cardinality micromegas cases are in brackets):

| case | before (ns) | after (ns) |
|---|---|---|
| `log/dyn_low_card/micromegas` | 52.6 | 49.7 |
| `log/dyn_high_card/micromegas` | 192.1 | 240.8 (standalone 176 to 183, old code standalone 188) |
| `log/dyn_high_card/micromegas_interop` | 400.5 | 496.1 (standalone 436 to 453, old code standalone 409) |
| `span/dyn_name/micromegas` | 112.5 | 115.4 |
| `contention/log/dyn_high_card/micromegas/8` | 7446 | 7612 |

The full-run `dyn_high_card` figures vary more between runs than the change does (the same code
gave 192 and 282 ns in two full runs), so the standalone re-runs are the better comparison.
`instrumentation-overhead.md` is regenerated from the final full run.

### `tracing` interop layer

With the merge rewrite in, `log/dyn_high_card/micromegas_interop` still measured 444 ns against
342 ns for OpenTelemetry. On the calling thread, about 55% of its time was in
`TracingCaptureLayer` before reaching Micromegas: each field was formatted twice (message and
property), each property value got its own `String`, and every event allocated a property `Vec`,
a `DynProperty` `Vec` and a growing message buffer. The layer now formats each field once into a
per-thread `values` buffer, keeps properties as ranges into it in a `SmallVec`, and reuses a
per-thread message buffer (fresh buffers when re-entered or during thread teardown, and buffers
over 4 KiB are not retained). Standalone medians:

| case | before (ns) | after (ns) |
|---|---|---|
| `log/static/micromegas_interop` | 140.2 | 90.1 |
| `log/formatted/micromegas_interop` | 194.6 | 105.3 |
| `log/dyn_high_card/micromegas_interop` | 444.5 | 277.4 (276 to 292 in repeat runs) |
| `log/dyn_high_card/otel` (unchanged, for reference) | 342.3 | 361 to 376 in repeat runs |

The final full run used for `instrumentation-overhead.md` has 263.9 ns for `micromegas_interop`
against 343.1 ns for `otel`. What is left on that path is interning the new value strings and
probing the set index (about 27%) plus the owned copy of the message.
