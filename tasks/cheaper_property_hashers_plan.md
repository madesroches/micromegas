# Cheaper Hashers in the Property Arena and Dependency Extraction Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1668

## Overview

The property arena and the log and metric dependency extraction hash with SipHash, the `std`
default, but none of these keys need DoS resistance. They are addresses, interned ids, and
property content that the process itself produced. This plan does three things:

- computes the arena content hash with xxh3
- keys the arena index and the extraction sets with FxHash (`rustc-hash`)
- in `extract`, skips the `desc` lookup when an event has the same `desc` as the previous event,
  and the set lookups when it has the same property set

The arena hash runs under the stream lock on every dynamic-property call, so it adds to
contention. `extract` runs on the sink's encoding thread for every event of every block. A
criterion run before and after the change measures the effect. The wire format, the
`EventSink` trait and the SQL surface don't change.

## Current State

**Arena** (`rust/tracing/src/property_arena.rs`):

- `content_hash` (`:44`) feeds every name and value into `std`'s `DefaultHasher` (SipHash-1-3).
  `str::hash` writes the bytes, then a `0xff` terminator.
- `get_or_insert` (`:67`) then looks the resulting `u64` up in `index: HashMap<u64, Vec<u32>>`
  (`:23`), which SipHashes it a second time.
- A hit compares content with `same_content`, so the hash only affects speed, never correctness.
- Callers are the four dynamic paths in `rust/tracing/src/dispatch.rs`:
  `tagged_float_metric_dyn`, `tagged_integer_metric_dyn`, `log_dyn_tagged_to_sink` and
  `log_interop_dyn_tagged`. Their `get_or_insert` calls are at `:775`, `:800`, `:968` and
  `:1002`, all while holding the stream lock.

**Extraction:**

- `LogMsgQueue::extract` (`rust/tracing/src/logs/block.rs:70`) and `MetricsMsgQueue::extract`
  (`rust/tracing/src/metrics/block.rs:68`) build a `HashSet<u64>` (SipHash).
- For every event, `record_*_event_dependencies` inserts the `desc` address. Tagged events also
  call `record_properties` (`rust/tracing/src/property_set.rs:144`), which inserts the set id.
  For a set seen for the first time, it also inserts the id of each name and value.
- So every event pays at least one SipHash insert, and a tagged event pays two, even when all
  of its dependencies were recorded long ago. The issue's scratch measurement puts this at
  about 54 ns per event.
- `ExtractDeps::extract` runs in `telemetry-sink/src/stream_block.rs:35`, on the sink side and
  not under the stream lock.

**Spans** (`rust/tracing/src/spans/block.rs`) use the same `HashSet<u64>` pattern but don't
share `record_properties`.

**Dependencies:**

- `xxhash-rust` 0.8 is already a workspace dependency (`rust/Cargo.toml:109`, features
  `xxh32`/`xxh64`), used by `analytics` and `perfetto`. It is BSL-1.0, which `rust/deny.toml`
  allows.
- `rustc-hash` 2.1.2 is already in `Cargo.lock` through DataFusion, but isn't a workspace
  dependency yet. It is MIT/Apache-2.0 and has no dependencies.
- `micromegas-tracing` lists its native-only dependencies under
  `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`. `property_arena` is native-only
  (`rust/tracing/src/lib.rs:164`), and so are the log and metric `block` modules
  (`#[cfg(not(target_arch = "wasm32"))] mod block;` in `logs/mod.rs:2` and `metrics/mod.rs:2`).

**Benchmark** (`rust/tracing/benches/properties.rs`, #1664):

- The `extract` group only covers miss-heavy blocks: `log/dyn_high_card`, `log/dyn_mixed` and
  `metric/dyn_high_card`. In those blocks every event brings a new set.
- No `extract` case covers the hit-dominated common case that the issue measured, or the
  repeated-callsite case the fast path targets.
- The single-thread `log/dyn_low_card` and `metric/dyn_low_card` cases (arena hits) and the
  `dyn_high_card` cases (arena misses) already cover `get_or_insert`.

## Design

### 1. Content hash: chained one-shot xxh3

```rust
fn content_hash<'a>(properties: impl Iterator<Item = (&'a str, &'a str)>) -> u64 {
    properties.fold(0, |h, (name, value)| {
        xxh3_64_with_seed(value.as_bytes(), xxh3_64_with_seed(name.as_bytes(), h))
    })
}
```

- Each string is hashed one-shot, seeded with the running hash. Names and values are short, and
  xxh3's one-shot paths for inputs of 16 bytes or less are a few multiplies. Nothing is buffered
  and there is no streaming state to set up.
- xxh3 mixes the input length into the hash, so `("ab", "c")` and `("a", "bc")` still hash
  differently without a terminator byte.
- Enable the `xxh3` feature only on tracing's own dependency line
  (`xxhash-rust = { workspace = true, features = ["xxh3"] }`).

### 2. Index and extraction sets: FxHash

- `PropertyArena::index` becomes `FxHashMap<u64, Vec<u32>>`.
- The extraction sets for logs and metrics become `FxHashSet<u64>`. `record_properties` and
  `record_log_event_dependencies` / `record_metric_event_dependencies` take
  `&mut FxHashSet<u64>`. All of them are crate-private, so no public API changes.

### 3. Repeated-event fast path in `extract`

In `LogMsgQueue::extract` and `MetricsMsgQueue::extract`, keep two locals: `last_desc: u64` and
`last_set: u64`, both starting at `0` (never a valid address). For each event:

- only call `record_*_event_dependencies` if `desc` differs from `last_desc`, then update it
- only call `record_properties` if the set differs from `last_set`, then update it

Every desc and set that passes the check still goes through the hash set, so correctness never
depends on the fast path. It only removes lookups that would have returned "already recorded".

- **Logs:** the desc check applies to `LogStaticStrEvent`, `LogStringEvent` and
  `TaggedLogString`. The set check applies to `TaggedLogString` and `TaggedLogInteropEvent`.
  Interop `target`/`msg` ids keep going straight to the set: they come from a different event
  shape, and the issue's case is the macro callsite.
- **Metrics:** the desc check applies to all four variants. The set check applies to the two
  tagged variants.

## Implementation Steps

### Phase 0: Baseline benchmark, before any change

1. In `rust/tracing/benches/properties.rs`, extend `bench_extraction_group` with hit-dominated
   blocks:
   - `log/static`: one desc. Exercises the desc fast path.
   - `log/set`: one desc and one interned set. Exercises both fast paths.
   - `log/dyn_low_card`: one desc and four arena sets in rotation. The set fast path misses
     every time, so this measures the hasher alone.
   - `metric/plain`: one desc, no set. Exercises the desc fast path.
   - `metric/set`: one desc and one interned set. Exercises both fast paths.
   - `metric/dyn_low_card`: same as `log/dyn_low_card`.

   Use 10 000 log events and 5 000 metric events, like the existing cases, so each block still
   fits in one block. Commit this bench-only change on its own.
2. Use a quiet machine. #1669 found run-to-run swings of 10-20% on some cases, so create a
   worktree at this commit to alternate runs in Phase 3. From the repo root:
   `git worktree add ../mm-hash-before HEAD`. The bench runs from `rust/` in each tree
   (`cargo bench -p micromegas-tracing --bench properties`), and the worktree's first run does a
   full build.

### Phase 1: Hashers

3. `rust/Cargo.toml`: add `rustc-hash = "2.1"` to `[workspace.dependencies]`.
4. `rust/tracing/Cargo.toml`: in the native-only section, add `rustc-hash.workspace = true` and
   `xxhash-rust = { workspace = true, features = ["xxh3"] }`.
5. `rust/tracing/src/property_arena.rs`: write `content_hash` with xxh3 (Design 1) and make
   `index` an `FxHashMap` (Design 2).
6. `rust/tracing/src/property_set.rs`, `logs/block.rs`, `metrics/block.rs`: change the extraction
   set type to `FxHashSet<u64>`.

### Phase 2: Fast path

7. `logs/block.rs` and `metrics/block.rs`: add the `last_desc` / `last_set` checks to `extract`
   (Design 3).

### Phase 3: Measure after

8. Run the bench on the branch, then in the worktree (`../mm-hash-before/rust`), then on the branch
   again, then in the worktree again. Keep the machine otherwise idle.
9. Add a Results section to this plan, with the same layout as
   `tasks/completed/stream_lock_critical_section_plan.md`:
   - the before and after medians for every case of both runs
   - the change in the mean

   The cases to watch are all `extract/*` (fast path and set hasher), single-thread
   `log/dyn_low_card`, `log/dyn_high_card`, `metric/dyn_low_card` and `metric/dyn_high_card`
   (content hash and index under the lock), and the `contention/*/dyn_high_card` rows. Put the
   headline numbers in the CHANGELOG entry. If a hasher change doesn't show up outside noise,
   say so rather than claiming it.

### Phase 4: Finish

10. Add the CHANGELOG entry. Run `cargo fmt`, then `python3 build/rust_ci.py` from the repo root.
    It runs the same native and wasm steps as CI (clippy, nextest, doc tests, machete, audit and
    deny for both lockfiles). `rust/datafusion-wasm` has its own tracked `Cargo.lock` that picks
    up the new dependencies, so commit the updated lockfile. Remove the worktree
    (`git worktree remove ../mm-hash-before`).

## Files to Modify

- `rust/Cargo.toml`: `rustc-hash` workspace dependency
- `rust/tracing/Cargo.toml`: `rustc-hash`, `xxhash-rust` with `xxh3`
- `rust/Cargo.lock`: `micromegas-tracing` dependency list
- `rust/datafusion-wasm/Cargo.lock`: new `xxhash-rust` package; `micromegas-tracing` dependency
  list gains `rustc-hash` and `xxhash-rust`
- `rust/tracing/src/property_arena.rs`: content hash, index hasher
- `rust/tracing/src/property_set.rs`: `record_properties` set type
- `rust/tracing/src/logs/block.rs`: set type, fast path
- `rust/tracing/src/metrics/block.rs`: set type, fast path
- `rust/tracing/benches/properties.rs`: hit-dominated `extract` cases
- `rust/tracing/tests/property_arena_tests.rs`: extraction dedup tests
- `CHANGELOG.md`

## Decisions

- Benchmarking before and after is a required step (user request). The baseline is taken on
  a bench-only commit before any tracing change, and before/after runs alternate.
- xxhash is used for the content hash (user suggestion). xxh3 over `FxHasher` on `str`: xxh3 is
  already a vetted workspace dependency and handles short strings better.
- `extract` uses a single last-event check rather than a small LRU: the target is a hot callsite
  that repeats the same desc and set.
- Spans extraction (`spans/block.rs`) and its `HashSet<u64>` are out of scope: the issue
  targets the property paths, and the bench has no span extraction case. Revisit with a span
  `extract` bench if the log/metric numbers show a clear gain.
- FxHash rather than an identity hasher: identity on aligned addresses leaves the low bits zero
  and gives every key the same top-7 hashbrown tag; FxHash costs about the same and also covers
  the index without a custom `Hasher`.
- The content hash stays under the stream lock: #1669 tried hashing before the lock
  (`MergedProperties`) and reverted it with no measurable difference.
- Moving the dedup to push time (#1654) is out of scope. This plan makes that move cheaper but
  doesn't depend on it.

## Documentation

- `CHANGELOG.md`: a Tracing entry under Unreleased with the before/after headline numbers. No
  Rust API break: all changed signatures are crate-private.
- No mkdocs change: none of the docs describe the hashing or the extraction internals.

## Testing Strategy

No-DB unit tests in `rust/tracing/tests/property_arena_tests.rs`. They reuse its existing
capturing sink and `DispatchGuard`. Each test captures one block and asserts
`block.events.extract().nb_objects()` (`HeterogeneousQueue`, with `ExtractDeps` imported). It does
not count variants through `.iter()`: `read_value` is `unimplemented!()` for
`StaticStringDependency` and `PropertySetDependency`, so iterating the deps queue panics. Keep N
to a few hundred so all events fit in one 64 KiB block. Rust doesn't guarantee that identical
string literals share a pointer, so assertions are relative to a reference block rather than
absolute totals:

Each test needs an anchor independent of the fast path, since the fast path can only omit
dependencies and a block and its reference lose the same ones.

- **Tagged events emit dependencies.** For logs and metrics, a block with one tagged event has
  more `nb_objects()` than one with one untagged event.
- **Repeated events emit dependencies once.** Log N events from one callsite with the same
  dynamic value, flush, and compare `nb_objects()` with a block from a single such event: they
  are equal. Do the same with an `..set` static set (own block) and for metrics. This checks the
  `FxHashSet` dedup, not the fast path.
- **Interleaved sets still dedup.** Log values A, B, A, B, A from one callsite that uses `PropertySet::empty()` with exactly one
  dynamic property and non-empty values A and B, which makes the
  fast path miss every time. Assert `nb(A,B,A,B,A) == nb(A) + 2` (set B costs its value and its
  set).
- **Interleaved callsites.** Alternate two log callsites that share one set. Assert
  `nb(two callsites) > nb(one callsite)` and equal to a block with a single event from each.

The existing arena tests (`same_content_same_pointer`, `different_values_different_sets`,
`split_between_static_and_dynamic_does_not_change_identity`) cover the content hash and index
behavior unchanged.

The performance effect is measured by the before/after criterion runs in Phases 0 and 3, not by
tests.
