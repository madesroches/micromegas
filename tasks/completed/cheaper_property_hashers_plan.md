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

## Results

Median time per call (per round of threads for `contention`, per block for `extract`),
`cargo bench -p micromegas-tracing --bench properties`, release profile, WSL machine, not
otherwise idle. The bench-only commit (before) and this branch (after) were run alternately,
twice each, with the same bench file. "Change" compares the mean of the two runs on each side.

| Case | before (run 1 / 2) | after (run 1 / 2) | Change (mean) |
|---|---|---|---|
| contention/log/dyn_high_card/1 | 353.07 ns / 343.77 ns | 314.68 ns / 311.10 ns | -10.2% |
| contention/log/dyn_high_card/4 | 3.9461 µs / 3.8872 µs | 3.4098 µs / 3.4929 µs | -11.9% |
| contention/log/dyn_high_card/8 | 10.375 µs / 9.7565 µs | 8.8713 µs / 9.0169 µs | -11.1% |
| contention/log/formatted/1 | 58.013 ns / 58.146 ns | 57.650 ns / 57.585 ns | -0.8% |
| contention/log/formatted/4 | 507.06 ns / 514.14 ns | 516.51 ns / 511.06 ns | +0.6% |
| contention/log/formatted/8 | 1.6774 µs / 1.6717 µs | 1.6892 µs / 1.6742 µs | +0.4% |
| contention/log/static/1 | 15.785 ns / 15.794 ns | 15.816 ns / 15.736 ns | -0.1% |
| contention/log/static/4 | 334.58 ns / 331.29 ns | 333.57 ns / 335.82 ns | +0.5% |
| contention/log/static/8 | 841.47 ns / 894.24 ns | 868.31 ns / 876.47 ns | +0.5% |
| contention/metric/dyn_high_card/1 | 337.48 ns / 328.84 ns | 260.50 ns / 303.34 ns | -15.4% |
| contention/metric/dyn_high_card/4 | 3.3302 µs / 2.7141 µs | 2.5156 µs / 2.7980 µs | -12.1% |
| contention/metric/dyn_high_card/8 | 7.8975 µs / 6.7901 µs | 5.9746 µs / 6.4341 µs | -15.5% |
| contention/metric/plain/1 | 9.9409 ns / 9.8927 ns | 9.9301 ns / 9.8987 ns | -0.0% |
| contention/metric/plain/4 | 203.31 ns / 202.96 ns | 191.90 ns / 192.32 ns | -5.4% |
| contention/metric/plain/8 | 504.41 ns / 513.54 ns | 529.31 ns / 506.65 ns | +1.8% |
| extract/log/dyn_high_card | 1.6331 ms / 1.2758 ms | 1.0223 ms / 1.0317 ms | -29.4% |
| extract/log/dyn_low_card | 501.45 µs / 502.79 µs | 320.63 µs / 326.12 µs | -35.6% |
| extract/log/dyn_mixed | 3.4422 ms / 2.8530 ms | 2.0275 ms / 2.0276 ms | -35.6% |
| extract/log/set | 496.45 µs / 500.72 µs | 258.91 µs / 259.47 µs | -48.0% |
| extract/log/static | 172.04 µs / 168.93 µs | 42.139 µs / 41.687 µs | -75.4% |
| extract/metric/dyn_high_card | 551.42 µs / 550.41 µs | 221.09 µs / 221.98 µs | -59.8% |
| extract/metric/dyn_low_card | 129.51 µs / 132.67 µs | 29.403 µs / 29.082 µs | -77.7% |
| extract/metric/plain | 78.089 µs / 78.840 µs | 19.004 µs / 18.787 µs | -75.9% |
| extract/metric/set | 135.85 µs / 131.87 µs | 19.025 µs / 18.925 µs | -85.8% |
| log/dyn_high_card | 344.20 ns / 346.86 ns | 310.97 ns / 305.99 ns | -10.7% |
| log/dyn_low_card | 86.535 ns / 92.484 ns | 69.129 ns / 68.870 ns | -22.9% |
| log/dyn_mixed | 1.6569 µs / 1.6246 µs | 1.7010 µs / 1.6872 µs | +3.3% |
| log/formatted | 57.526 ns / 58.042 ns | 57.733 ns / 58.003 ns | +0.1% |
| log/literals | 44.097 ns / 44.278 ns | 44.815 ns / 44.520 ns | +1.1% |
| log/set | 43.213 ns / 43.096 ns | 44.059 ns / 43.392 ns | +1.3% |
| log/static | 15.959 ns / 15.986 ns | 15.989 ns / 15.822 ns | -0.4% |
| metric/dyn_high_card | 594.24 ns / 541.39 ns | 594.01 ns / 546.94 ns | +0.5% |
| metric/dyn_low_card | 66.865 ns / 68.659 ns | 55.643 ns / 55.344 ns | -18.1% |
| metric/literals | 12.018 ns / 12.125 ns | 12.099 ns / 11.994 ns | -0.2% |
| metric/literals_set | 163.95 ns / 152.42 ns | 117.42 ns / 138.52 ns | -19.1% |
| metric/plain | 9.8559 ns / 10.031 ns | 10.090 ns / 9.9967 ns | +1.0% |
| metric/set | 11.230 ns / 11.284 ns | 11.187 ns / 11.175 ns | -0.7% |

Reading the numbers:

- `extract` improves on every case: -75% to -86% where the desc and set fast path hits
  (`log/static`, `metric/plain`, `metric/set`, `metric/dyn_low_card`), -48% on `log/set`, and
  -30% to -36% on the cases that miss the fast path and only gain from FxHash
  (`log/dyn_low_card`, `log/dyn_high_card`, `log/dyn_mixed`; `metric/dyn_high_card` -60%).
- Content hash and index under the lock: single-thread `log/dyn_low_card` -23%,
  `metric/dyn_low_card` -18%, `log/dyn_high_card` -11%, and the `contention/*/dyn_high_card`
  rows -10% to -16% at 1, 4 and 8 threads.
- `metric/dyn_high_card`, single thread, and `metric/literals_set` swing 10-20% between runs of
  the same code, so their means are inside noise and are not claimed as wins.
- `log/dyn_mixed` (+3%), `log/set`, `log/literals` and the static/plain/formatted cases are
  unchanged within noise.
