# Cheaper Hashers in the Property Arena and Dependency Extraction Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1668

## Overview

The property arena and the log and metric dependency extraction hash with SipHash, the `std`
default, but none of these keys need DoS resistance. They are addresses, interned ids, and
property content that the process itself produced. This plan does three things:

- computes the arena content hash with xxh3
- keys the arena index and the extraction sets with FxHash (`rustc-hash`)
- in `extract`, skips the set lookups when an event has the same `desc` and property set as the
  previous one

The arena hash runs under the stream lock on every dynamic-property call, so it adds to
contention. `extract` runs on the sink's encoding thread for every event of every block. A
criterion run before and after the change measures the effect. The wire format, the
`EventSink` trait and the SQL surface don't change.

## Current State

**Arena** (`rust/tracing/src/property_arena.rs`):

- `content_hash` (`:44`) feeds every name and value into `std`'s `DefaultHasher` (SipHash-1-3).
  `str::hash` writes the bytes, then a `0xff` terminator.
- `get_or_insert` (`:66`) then looks the resulting `u64` up in `index: HashMap<u64, Vec<u32>>`
  (`:24`), which SipHashes it a second time.
- A hit compares content with `same_content`, so the hash only affects speed, never correctness.
- Callers are the four dynamic paths in `rust/tracing/src/dispatch.rs`:
  `tagged_float_metric_dyn` (`:775`), `tagged_integer_metric_dyn` (`:800`),
  `log_dyn_tagged_to_sink` (`:968`) and `log_interop_dyn_tagged` (`:1002`). All of them call
  `get_or_insert` while holding the stream lock. #1669 tried hashing before the lock
  (`MergedProperties`) and reverted it: it made no measurable difference, so the hash stays
  under the lock and it has to be cheap.

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
  `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`. `property_arena` and the block
  modules are native-only (`rust/tracing/src/lib.rs:164`).

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
  differently without a terminator byte. A collision would only cost one extra `same_content`
  compare anyway.
- Enable the `xxh3` feature only on tracing's own dependency line
  (`xxhash-rust = { workspace = true, features = ["xxh3"] }`).

### 2. Index and extraction sets: FxHash

- `PropertyArena::index` becomes `FxHashMap<u64, Vec<u32>>`.
- The extraction sets for logs and metrics become `FxHashSet<u64>`. `record_properties` and
  `record_log_event_dependencies` / `record_metric_event_dependencies` take
  `&mut FxHashSet<u64>`. All of them are crate-private, so no public API changes.

The issue suggests an identity hasher for these maps. That's a fine fit for the index, but a
bad one for address keys: hashbrown picks the bucket from the low bits of the hash and the
SIMD control tag from the top 7 bits. Aligned addresses have zero low bits, and user-space
addresses share their top bits, so every key would get the same tag and lookups would fall
back to full key compares. FxHash is one multiply and a rotate, which costs about the same as
identity and spreads both ends. Using FxHash for the index too avoids writing a custom
`Hasher`.

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
   - `metric/plain`
   - `metric/dyn_low_card`

   Use 10 000 log events and 5 000 metric events, like the existing cases, so each block still
   fits in one block. Commit this bench-only change on its own.
2. Use a quiet machine. #1669 found run-to-run swings of 10-20% on some cases, so create a
   worktree at this commit (`git worktree add ../../mm-hash-before HEAD`) to alternate runs in
   Phase 3.

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

8. Run the bench on the branch, then in the worktree (`mm-hash-before`), then on the branch again,
   then in the worktree again. Use the same bench file on both sides, and keep the machine
   otherwise idle.
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

10. Add the CHANGELOG entry. Run `cargo fmt`, then `cargo clippy --workspace -- -D warnings`,
    then `cargo test -p micromegas-tracing`, then `cargo deny check licenses bans sources` (from
    `rust/`; new direct dependency). `rust/datafusion-wasm` has its own tracked `Cargo.lock`
    that picks up the new dependencies, so run `python3 build/rust_ci.py wasm` (from the repo
    root) and, from `rust/datafusion-wasm`,
    `cargo deny --config ../deny.toml check licenses bans sources --allow unnecessary-skip`,
    then commit the updated lockfile. Remove the worktree.

## Files to Modify

- `rust/Cargo.toml`: `rustc-hash` workspace dependency
- `rust/tracing/Cargo.toml`: `rustc-hash`, `xxhash-rust` with `xxh3`
- `rust/datafusion-wasm/Cargo.lock`: new `xxhash-rust` entry
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
- Moving the dedup to push time (#1654) is out of scope. This plan makes that move cheaper but
  doesn't depend on it.

## Documentation

- `CHANGELOG.md`: a Tracing entry under Unreleased with the before/after headline numbers. No
  Rust API break: all changed signatures are crate-private.
- No mkdocs change: none of the docs describe the hashing or the extraction internals.

## Testing Strategy

No-DB unit tests in `rust/tracing/tests/property_arena_tests.rs`. They reuse its existing
capturing sink and `DispatchGuard`. Each test captures one block and asserts
`block.events.extract().nb_objects()` (`HeterogeneousQueue`, with `ExtractDeps` imported) against
an expected total. It does not count variants through `.iter()`: `read_value` is
`unimplemented!()` for `StaticStringDependency` and `PropertySetDependency`, so iterating the
deps queue panics. Keep N to a few hundred so all events fit in one 64 KiB block. Expected
totals:

- A log desc contributes one `Utf8StaticStringDependency` per distinct pointer among `fmt_str`,
  `target`, `module_path` and `file` (at most 4), plus one `LogMetadataRecord`. A metric desc
  does the same over `name`, `unit`, `target` and `file`, plus one `MetricMetadataDependency`.
- Each distinct property set contributes one `StaticStringDependency` per distinct name or value
  pointer not already recorded, plus one `PropertySetDependency`.

- **Repeated events emit dependencies once.** Log N events from one callsite with the same
  dynamic value, then N with an `..set` static set. The total equals one desc plus one set.
  Do the same for metrics.
- **Interleaved sets still dedup.** Log values A, B, A, B, A from one callsite, which makes the
  fast path miss every time. The total equals one desc plus two sets, with each value string
  counted once. This catches a fast path that skips the set entirely, or one whose `last_*` is
  updated wrong.
- **Interleaved callsites.** Alternate two log callsites that share one set. The total equals
  two descs plus one set.

The existing arena tests (`same_content_same_pointer`, `different_values_different_sets`,
`split_between_static_and_dynamic_does_not_change_identity`) cover the content hash and index
behavior unchanged.

The performance effect is measured by the before/after criterion runs in Phases 0 and 3, not by
tests.
