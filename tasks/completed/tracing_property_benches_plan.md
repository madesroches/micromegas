# Tracing Property Benchmarks Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1664

## Overview
Add a criterion bench to `rust/tracing` that measures enabled log and metric calls, including
the property shapes listed below (literals, `..set`, dynamic values of low and high
cardinality), the cost of `ExtractDeps::extract` on blocks holding high-cardinality properties,
and contention when several threads log at once. Today's benches measure no logging work at all,
so there is no baseline for the upcoming property-path optimizations.

## Current State
- `rust/tracing/benches/with_dispatch.rs` installs `NullEventSink`
  (`rust/tracing/src/event/sink.rs:46`), whose `on_log_enabled` returns `false`. It also never
  calls `set_max_level`, and `MAX_LEVEL_FILTER` starts at `0` (Off,
  `rust/tracing/src/levels.rs:702`), so `error!("test")` returns at the macro's level check
  (`__log_call!`, `rust/tracing/src/macros.rs:527`) before reaching dispatch. `dispatch/log`
  therefore measures one relaxed atomic load (`max_level()`).
- `dispatch/metric` measures `imetric!` without properties. No bench covers properties.
- Macro paths (`rust/tracing/src/macros.rs:421-605`):
  - no properties → `dispatch::log` / `int_metric`
  - literals only → per-callsite interned set (`__callsite_set!`) → `log_tagged` /
    `tagged_integer_metric` (via the `_dyn` entry point with an empty slice)
  - `..set` only → same as above with the spread set
  - literals + `..set` → every pair becomes a `DynProperty`, merged into the block arena
  - any dynamic value → gated on `log_enabled` for logs, then in `log_dyn_tagged` /
    `tagged_integer_metric_dyn` (`rust/tracing/src/dispatch.rs:908`, `:773`), via
    `merge_properties` + `PropertyArena::get_or_insert`
    (`rust/tracing/src/property_arena.rs:31`, `:67`)
- Extraction: `ExtractDeps for LogMsgQueue` (`rust/tracing/src/logs/block.rs:67`) and
  `for MetricsMsgQueue` (`rust/tracing/src/metrics/block.rs:65`) walk the queue and call
  `record_properties` for tagged events.
- Blocks reach the sink through `on_process_log_block` / `on_process_metrics_block` as
  `Arc<LogBlock>` / `Arc<MetricsBlock>` on flush, either when a block fills or on
  `flush_log_buffer()` / `flush_metrics_buffer()` (`rust/tracing/src/dispatch.rs:209`, `:229`).

## Design
One new bench target, `rust/tracing/benches/properties.rs` (`harness = false`), with one
process-wide dispatch init (the global dispatch can only be initialized once per process).

### Bench sink
A `BenchSink` defined in the bench file:
- `on_log_enabled` returns `true`; `on_log` does nothing.
- `on_process_log_block` / `on_process_metrics_block` drop the block, unless a `capture:
  AtomicBool` is set, in which case they push it onto a `Mutex<Vec<Arc<LogBlock>>>` /
  `Mutex<Vec<Arc<MetricsBlock>>>`. Capture is only used by the extraction group.
- Every other callback is a no-op; `is_busy` returns `false`.

Setup: `TracingSystemGuard::new(10 MiB, 1 MiB, 10 MiB, sink, HashMap::new(), false)` (the
`telemetry-sink` defaults) and `set_max_level(LevelFilter::Trace)`. A single `criterion_benchmark`
function binds `let _tracing_guard = TracingSystemGuard::new(..).expect("init telemetry");` and
runs all four groups before returning (as `with_dispatch.rs` does), since dropping the guard
swaps the sink to `NullEventSink` and re-init fails.

Shared inputs: `static LABELS: [&str; 4]`, a `&'static PropertySet` built once with
`PropertySet::find_or_create(vec![Property::new(..), ..])` holding three pairs `x = "1"`,
`y = "2"`, `z = "3"` (keys not clashing with `tier`, `l`, `id`, `a`, `b`, `c`), and a `usize` counter `i` incremented inside each iteration so
`id = %i` produces a new value every call. In the log/metric groups the counter lives outside `bench_function` (captured by the outer closure) so it persists across criterion warm-up and sample rounds instead of restarting at 0 and hitting the per-block arena dedup.

### Groups and cases

`log` group (`b.iter`):
| id | call |
|---|---|
| `static` | `info!("static")` |
| `formatted` | `info!("value {}", i)` |
| `literals` | `info!(a = "1", b = "2", c = "3", "static")` |
| `set` | `info!(..set, "static")` |
| `dyn_low_card` | `info!(l = LABELS[i % 4], "static")` |
| `dyn_high_card` | `info!(id = %i, "static")` |
| `dyn_mixed` | `info!(a = "1", b = "2", c = "3", id = %i, "static")` |

`metric` group (`b.iter`):
| id | call |
|---|---|
| `plain` | `imetric!("name", "unit", 1)` |
| `literals` | `imetric!("name", "unit", tier = "ram", 1)` |
| `set` | `imetric!("name", "unit", ..set, 1)` |
| `literals_set` | `imetric!("name", "unit", tier = "ram", ..set, 1)` |
| `dyn_low_card` | `imetric!("name", "unit", l = LABELS[i % 4], 1)` |
| `dyn_high_card` | `imetric!("name", "unit", id = %i, 1)` |

`extract` group — for `log/dyn_high_card`, `log/dyn_mixed` and `metric/dyn_high_card`:
1. Flush, set `capture`, emit `N = 10_000` events of that case (`N = 5_000` for
   `metric/dyn_high_card`), flush, clear `capture`, take the captured blocks and assert there is
   exactly one.
2. Print, with `println!` before benchmarking, `object bytes/event = block.events.len_bytes() /
   nb_objects` and `dependency bytes/event = extract().len_bytes() / nb_objects` (criterion has
   no custom-metric output; a printed line is the simplest report). The bench imports
   `micromegas_transit::HeterogeneousQueue` (for `len_bytes()` / `nb_objects()`) and
   `micromegas_tracing::event::ExtractDeps`; transit is already a regular dependency of tracing.
3. Bench `block.events.extract()` with `Throughput::Elements(nb_objects)`; `time:` is per
   `extract()` call and criterion's `thrpt:` reports events/s.

`contention` group — for `log/static`, `log/dyn_high_card`, `metric/plain`,
`metric/dyn_high_card`, at 1, 4 and 8 threads (`BenchmarkId::new(case, threads)`):
`b.iter_custom(|iters| ...)` spawns `threads` scoped threads that each wait on a
`Barrier(threads + 1)`, then perform `iters` calls with a thread-local counter over a disjoint range (thread `t` runs
`for i in base + t * iters..base + (t + 1) * iters { case(i as usize) }`, with `base` kept outside `bench_function` and advanced by `threads * iters` after each `iter_custom` call), so `id = %i` is unique across threads
with no shared atomic; the main thread waits on the barrier, starts
an `Instant`, and joins. It returns the elapsed time. `Throughput::Elements(threads)` makes
criterion report aggregate calls/s alongside the per-round latency.

Each case is one non-capturing closure taking the counter (e.g. `|i: usize| info!(id = %i,
"static")`, with `set` from a static), called by the `log`/`metric` bench and, where listed, the
`extract` and `contention` groups, so each call expression is written once.

## Implementation Steps
1. Create `rust/tracing/benches/properties.rs` with `BenchSink`, setup, and the four groups.
2. Add to `rust/tracing/Cargo.toml`:
   ```toml
   [[bench]]
   name = "properties"
   harness = false
   ```
3. Run `cargo clippy --all-targets` and `cargo fmt` from `rust/`.

## Files to Modify
- `rust/tracing/benches/properties.rs` (new)
- `rust/tracing/Cargo.toml`

## Trade-offs
- **New bench target vs. extending `with_dispatch.rs`.** A new file keeps `with_dispatch`'s
  numbers comparable with history and avoids mixing a disabled-sink bench with an enabled one in
  one process (the sink is fixed at init). `with_dispatch` stays as the "dispatch installed, logs
  disabled" measurement.
- **Capture flag on one sink vs. a second process for extraction.** Dispatch can only be
  initialized once per process without `force_uninit` (unsafe, test-only); a flag is simpler.
- **Building extraction blocks through dispatch vs. constructing them by hand.** Going through
  the macros produces exactly the events and arena contents production does.

## Documentation
None; benches are not documented in `mkdocs/`.

## Testing Strategy
This change is itself a benchmark; there is no logic to unit-test. The single-block assertion in
the extraction setup guards against measuring a partial block. Bench compilation is
checked only locally by Implementation Step 3's `cargo clippy --all-targets`; CI does not build
benches.

## Manual Verification
1. `cd rust && cargo bench -p micromegas-tracing --bench properties -- --quick` — every group
   runs; `log/static` reports tens of ns (proving logs are enabled); the extraction section
   prints object and dependency bytes per event for the three cases. Bench output is read by a
   human, so it is not automated.
