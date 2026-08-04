# Per-Query Peak Memory Attribution Plan

Issue: [#1406](https://github.com/madesroches/micromegas/issues/1406)

## Overview

Answer "which SQL queries need too much memory?" by giving every FlightSQL query its own thin
`MemoryPool` wrapper over the process-shared pool, and reporting that wrapper's peak in the existing
per-query audit record. The pool *instance* becomes the query identity, so attribution needs no
names, no ambient context, and no `HashMap` — a reservation is welded to the pool it registered
with, whatever task or thread later grows it. A per-operator breakdown is explicitly not a goal.

## Current State

### One pool for the whole process

`make_runtime_env()` (`rust/analytics/src/lakehouse/runtime.rs:9-27`) builds a single `RuntimeEnv`
wrapping either `GreedyMemoryPool` (when `MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB` is set) or
`UnboundedMemoryPool` in a `TrackConsumersPool` with `nb_top_consumers = 5`.

That `Arc<RuntimeEnv>` is stored once in `LakehouseContext::runtime`
(`rust/analytics/src/lakehouse/lakehouse_context.rs:26`, built at `:41` and `:56`) and handed out by
`LakehouseContext::runtime()` (`:115`) to every session context in the process:

- `make_session_context` (`rust/analytics/src/lakehouse/query.rs:221`) — the FlightSQL path
- `merge.rs:400`, `jit_partitions.rs:77/129/285/430`, `partition_source_data.rs:271` — maintenance
  and JIT materialization
- `flight_sql_server.rs:196/198/214`, `monolith/src/main.rs:316`,
  `telemetry-maintenance-srv/src/main.rs:37` — process-level view-factory setup

`TrackConsumersPool`'s per-consumer peak/reserved tracking therefore mixes every concurrent query.

### Why the obvious workarounds don't work (verified against datafusion 54.1)

1. **Entries vanish at `unregister`.** `TrackConsumersPool::unregister` does
   `self.tracked_consumers.lock().remove(&consumer.id())`
   (`datafusion-execution-54.1.0/src/memory_pool/pool.rs:546-548`), so polling `metrics()` before and
   after a span misses any consumer whose whole lifetime fits between polls.
2. **Consumer names aren't query-scoped.** They are generic per-operator strings with a partition
   index (`ExternalSorter[3]`, `ExternalSorterMerge[0]`, `AggregateStream[0]`), so two concurrent
   queries' consumers are indistinguishable.
3. **No hook to stamp a query tag.** `MemoryConsumer` is `{name, can_spill, id}` with no user-data
   field, and the consumers that matter are all constructed *inside* DataFusion (e.g.
   `sorts/sort.rs:283-289`, `aggregates/row_hash.rs`, `topk/mod.rs`, `repartition/mod.rs`,
   `joins/*`). Nothing we control sits between `MemoryConsumer::new(...)` and `.register(pool)`.
   Recovering the query from ambient context is unreliable: `tokio::task_local!` isn't inherited
   across `tokio::spawn`, DataFusion spawns internally, and join build sides register inside lazily
   polled `OnceFut`s, so *which* task registers them depends on who polls first. Thread-locals are
   worse — the multi-thread runtime work-steals.

### Where per-query cost is already reported

`QueryAuditRecord` (`rust/public/src/servers/query_audit.rs:46-79`) is one JSON log line per
FlightSQL query under the `flightsql_query_audit` target, carrying full SQL text plus stage timings,
`output_rows` and `bytes_scanned`. `QueryAuditState`
(`rust/public/src/servers/flight_sql_service_impl.rs:82-149`) is built as soon as attribution
resolves (`:350-367`), so even setup failures emit a record; `emit()` takes `&self` and is called
from the setup-error `map_err` closures, from both `CompletionTrackedStream::poll_next` terminal arms
(`:203`, `:217`), and from its `Drop` impl for abandoned streams (`:178-182`).

`execute_query` is the single funnel for query execution: `do_get_fallback` (`:497`) and the
statement ticket path (`:658`) both delegate to it. `lakehouse::query::query()` (the non-streaming
`df.collect()` variant) has no production caller — only `analytics/tests/thread_spans_ordering_db_test.rs`.

Documentation lives at `mkdocs/docs/query-guide/query-audit-log.md`; unit tests at
`rust/public/tests/query_audit_tests.rs`.

## Design

### `ScopedMemoryPool`

A per-query wrapper delegating to the shared pool. Chain:

```
per-query ScopedMemoryPool  ->  shared TrackConsumersPool  ->  shared Greedy/UnboundedMemoryPool
      (accounting only)            (top-consumer OOM msgs)         (the global budget)
```

Only the accounting object is per query. The budget, disk manager, caches and object-store registry
stay shared.

```rust
// rust/analytics/src/lakehouse/scoped_memory_pool.rs
pub struct ScopedMemoryPool {
    inner: Arc<dyn MemoryPool>,
    current: AtomicUsize,
    peak: AtomicUsize,
}

impl ScopedMemoryPool {
    pub fn new(inner: Arc<dyn MemoryPool>) -> Self { /* zeroed counters */ }
    /// Monotonic high-water mark of tracked reservation for this query, in bytes.
    pub fn peak(&self) -> usize { self.peak.load(Ordering::Relaxed) }
    /// Currently outstanding tracked reservation; 0 at quiescence.
    pub fn current(&self) -> usize { self.current.load(Ordering::Relaxed) }
}

impl MemoryPool for ScopedMemoryPool {
    fn name(&self) -> &str { "scoped" }

    // MUST forward: the trait's default `register`/`unregister` are no-ops, and
    // TrackConsumersPool's whole HashMap (and therefore the top-consumers text in
    // its OOM error) is driven by them.
    fn register(&self, c: &MemoryConsumer) { self.inner.register(c) }
    fn unregister(&self, c: &MemoryConsumer) { self.inner.unregister(c) }

    fn grow(&self, r: &MemoryReservation, additional: usize) {
        self.inner.grow(r, additional);
        self.record_grow(additional);
    }

    fn try_grow(&self, r: &MemoryReservation, additional: usize) -> Result<()> {
        self.inner.try_grow(r, additional)?;   // `?` keeps counters untouched on failure
        self.record_grow(additional);
        Ok(())
    }

    fn shrink(&self, r: &MemoryReservation, shrink: usize) {
        self.inner.shrink(r, shrink);
        let prev = self.current.fetch_sub(shrink, Ordering::Relaxed);
        debug_assert!(prev >= shrink, "scoped pool shrink underflow");
    }

    fn reserved(&self) -> usize { self.inner.reserved() }        // global truth, unchanged semantics
    fn memory_limit(&self) -> MemoryLimit { self.inner.memory_limit() }
}

// record_grow:
//   let cur = self.current.fetch_add(additional, Ordering::Relaxed) + additional;
//   self.peak.fetch_max(cur, Ordering::Relaxed);
```

Plus `Debug` (derive, but `Arc<dyn MemoryPool>` is `Debug` so derive works) and a `Display` impl —
the `MemoryPool` trait requires `Any + Send + Sync + Debug + Display`
(`memory_pool/mod.rs:186`). Mirror `TrackConsumersPool`'s style (`pool.rs:414-424`):
`write!(f, "scoped(inner_pool: {}, peak: {})", self.inner, self.peak())`.

Notes on the mechanics:

- `record_grow` uses `fetch_add`'s return value rather than re-loading `current`, which is tighter
  than DataFusion's own `TrackedConsumer::grow` (`pool.rs:342-345`, which re-reads and can therefore
  record a peak influenced by a concurrent grow).
- `reserved()` deliberately delegates instead of returning `current`. Nothing in DataFusion's
  non-test code reads `reserved()`, but `memory_limit()` is read by
  `memory_pool/arrow.rs:80` and `runtime_env.rs:261`; delegating both keeps spilling and
  limit-reporting behavior byte-identical to today.
- `shrink` underflow is structurally unreachable (see below), so a `debug_assert` is the right level
  — a CAS loop with `saturating_sub` on the hot path would not be.

### Concurrent isolation is structural, not contextual

`MemoryConsumer::register(self, pool: &Arc<dyn MemoryPool>)`
(`memory_pool/mod.rs:339-345`) stores that exact `Arc<dyn MemoryPool>` in the reservation's
`SharedRegistration` (`:356`, `:373-374`), and every later mutation routes through it:
`self.registration.pool.shrink` (`:394`, `:414`, `:435`), `.grow` (`:464`), `.try_grow` (`:472`).
`split()` (`:487`) and `new_empty()` (`:502`) clone the same `Arc<SharedRegistration>`, so derived
reservations stay bound to the same pool.

A reservation is therefore permanently welded to the pool instance it registered with. Query B's
operators hold a different `Arc` and cannot reach query A's counters — work-stealing, DataFusion's
internal `tokio::spawn`s, and lazily-polled join build sides are all irrelevant because nothing is
inferred from context. The same property makes `fetch_sub` underflow unreachable: a shrink always
lands on the pool that saw its grow.

### Wiring: scope at `LakehouseContext`

`RuntimeEnvBuilder::from_runtime_env(shared)` (`runtime_env.rs:496-522`) is exactly the right
primitive — verified that it reuses rather than rebuilds: `DiskManagerConfig::Existing` returns the
*same* `Arc<DiskManager>` (`disk_manager.rs:197-199`), and `CacheManager::try_new` `Arc::clone`s the
existing statistics / list-files / file-metadata caches (`cache/cache_manager.rs:358-402`). Per-query
cost is one small `CacheManager` allocation plus a handful of Arc bumps.

```rust
// rust/analytics/src/lakehouse/runtime.rs
/// Builds a `RuntimeEnv` that reuses `shared`'s disk manager, caches and object-store
/// registry but installs `scoped_pool` (already wrapping `shared`'s memory pool) as its
/// memory pool. Takes the pool as a parameter, rather than constructing it internally, so
/// callers can hand the (infallible) pool to `QueryAuditState` before this fallible step runs.
pub fn scoped_runtime(
    shared: &RuntimeEnv,
    scoped_pool: Arc<ScopedMemoryPool>,
) -> Result<Arc<RuntimeEnv>>;
```

Scope at `LakehouseContext`, **not** inside `make_session_context`:

```rust
// rust/analytics/src/lakehouse/lakehouse_context.rs
/// Clones this context with `runtime` swapped, sharing the metadata cache and reader factory.
pub fn with_runtime(&self, runtime: Arc<RuntimeEnv>) -> Arc<Self> {
    Arc::new(Self { runtime, ..self.clone() })
}
```

This must be a struct-update clone, not a call to `new()`/`with_caches()` — those rebuild the
`MetadataCache` and `ReaderFactory` (`lakehouse_context.rs:60-104`), which would throw away the
shared metadata cache per query.

Scoping at the context level is what makes micromegas' own *nested* queries inherit the scope
automatically. `PerfettoTraceExecutionPlan` captures `Arc<LakehouseContext>` at construction
(`perfetto_trace_execution_plan.rs:44`) and builds a fresh session context during execution (`:232`);
`parse_block_table_function.rs:81` (`fetch_block_metadata`) does the same. Both receive the outer
query's context through `register_functions`, so their memory counts toward the parent query instead
of vanishing into the process pool. Same for JIT materialization —
`jit_partitions.rs:77/129/285/430` all go through `lakehouse.runtime()`.

Then in `execute_query` (`flight_sql_service_impl.rs:369-384`): construct the `ScopedMemoryPool`
(infallible — it's just `ScopedMemoryPool::new(...)` over the shared pool) *before* `audit_state`,
so `audit_state` can own it from the start; build the `RuntimeEnv` (fallible) after, with the same
`map_err(...).emit("error", ...)` pattern as every other setup stage:

```rust
let scoped_pool = Arc::new(ScopedMemoryPool::new(self.lakehouse.runtime().memory_pool.clone()));
let mut audit_state = QueryAuditState { pool: scoped_pool.clone(), /* unchanged */ .. };
// ...
let scoped_env = scoped_runtime(self.lakehouse.runtime(), scoped_pool.clone()).map_err(|e| {
    audit_state.emit("error", Some(format!("error building scoped runtime: {e}")));
    status!("error building scoped runtime", e)
})?;
let lakehouse = self.lakehouse.with_runtime(scoped_env);
let ctx = make_session_context(lakehouse, /* unchanged */ ...).await?;
```

`do_action_create_prepared_statement` (`:843`) stays on the shared context — it only plans, never
executes, and emits no audit record.

### Reporting

Add to `QueryAuditRecord` (`query_audit.rs`):

```rust
pub peak_memory_bytes: u64,
pub spilled_bytes: u64,
pub spill_count: u64,
```

`QueryAuditState` gains `pool: Arc<ScopedMemoryPool>`; `emit()` reads `self.pool.peak() as u64`.
No `Drop` machinery is needed: `peak` is a monotonic max, so it is correct at emit time even when
every reservation has already been released. Reading it from a different thread than the one that
grew it is fine — `emit` on the success path runs after the stream yielded `Ready(None)`, which
already establishes happens-before through the task scheduler, and on the error/`Drop` paths a
slightly stale relaxed read of a monotonic counter is acceptable for a diagnostic.

Spill metrics come from the plan tree, extending `ScanMetrics` / `aggregate_scan_metrics`
(`query_audit.rs:13-39`) with a second accumulator in the same walk. **`sum_by_name` cannot be used
here**: `MetricsSet::sum_by_name` explicitly returns `false` for `MetricValue::SpillCount` and
`SpilledBytes` (`datafusion-physical-expr-common-54.1.0/src/metrics/mod.rs:298-315`), so it would
silently report zero. Use the dedicated accessors `MetricsSet::spill_count()` /
`MetricsSet::spilled_bytes()` (`:245-254`), summed over the tree.

Spilling only happens once `MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB` is set — unset today, so the
inner pool is `UnboundedMemoryPool` and nothing spills or fails. `peak_memory_bytes` is precisely
what tells us where to set that budget; `spilled_bytes`/`spill_count` then distinguish queries that
actually hit the limit from those that merely came close.

Also emit `imetric!("query_peak_memory_bytes", "bytes", peak)` for dashboards — low cardinality,
no `PropertySet` needed. Cheap and additive; include it. It must be emitted from
`QueryAuditState::emit()` itself — not from the two `CompletionTrackedStream::poll_next` terminal
arms, `:204` and `:218`. `emit()` is the single `&self` method called from every terminal path
(the two `poll_next` arms, the `Drop` impl for abandoned streams with status `"incomplete"`, and
the setup-error `map_err` closures), so putting the `imetric!` there means error and incomplete
records contribute a sample too, typically ~0 for setup failures since the pool has barely been
used at that point. That is the intended, uniform behavior: every audited query — success, error,
or incomplete — reports one `query_peak_memory_bytes` sample.

### Scope and limits of the metric

`peak` is the peak of *tracked* reservation — a lower bound on process cost. Not counted:

- in-flight `RecordBatch`es and parquet decode buffers (DataFusion documents this as deliberate);
- the L1 byte cache and micromegas' `MetadataCache` (separately accounted);
- `AsyncArrowWriter` row-group buffers in `write_partition_from_rows`
  (`write_partition.rs:676`), which is what JIT materialization inside a query uses;
- the non-streaming `query()` path's full `df.collect()` buffer (`query.rs:277`) — no production
  caller anyway.

For ad-hoc SQL the metric is well matched regardless: sorts, hash joins, grouped aggregates and TopK
are where an expensive query blows up, and those all register consumers. Note that any query with a
`SortExec` that reaches its spill check will show at least
`datafusion.execution.sort_spill_reservation_bytes` (default 10 MB) per partition — the sorter grows
its merge reservation by that amount in `sort_or_spill_in_mem_batches` (`sorts/sort.rs:714-719`).

`peak_memory_bytes` and `spilled_bytes`/`spill_count` do not share the same scope. The peak comes
from the `ScopedMemoryPool` instance, so it naturally includes nested session contexts built during
execution (Perfetto trace queries, `fetch_block_metadata`, JIT materialization) — that inheritance is
the whole point of scoping at `LakehouseContext` rather than per-session. The spill counters instead
come from walking `plan.children()` of the *outer* physical plan in `aggregate_scan_metrics`
(`query_audit.rs:22-32`); a nested session context built inside a leaf node (e.g.
`perfetto_trace_execution_plan.rs:232`, `parse_block_table_function.rs:81`,
`process_spans_table_function.rs:254`) is an opaque leaf in that tree, so any spill inside it is
invisible to the sum. A query can therefore legitimately show `peak_memory_bytes > 0` with
`spilled_bytes == 0` even when nested work spilled — this is not a bug, just a narrower scope for the
spill counters than for the peak.

Anything built from an unscoped `LakehouseContext` (maintenance merges, `export_log_view.rs`,
process-level view-factory setup) lands on the shared pool only. The wrapper is purely additive:
`shared.reserved()` stays ground truth and `sum(scoped current) <= shared.reserved()`. The gap is
memory nobody claimed, never memory charged to the wrong query.

### Cost

Two relaxed atomic RMWs per `grow`/`shrink`, on top of the *process-global* mutex that
`TrackConsumersPool` already takes on every one of those calls (`pool.rs:550-594`) — strictly cheaper
than what already runs. Memory is roughly 1 KB per in-flight query (pool + `RuntimeEnv` +
`CacheManager`), with nothing accumulating per consumer.

## Implementation Steps

1. **`ScopedMemoryPool`** — new file `rust/analytics/src/lakehouse/scoped_memory_pool.rs` (~70
   lines) with the impl above, including `register`/`unregister` forwarding, `name`, `Debug`,
   `Display`, `peak()`, `current()`. Declare `pub mod scoped_memory_pool;` in
   `rust/analytics/src/lakehouse/mod.rs` (alphabetical, between `retire_partitions_table_function`
   and `session_configurator`), with a one-line doc comment matching the surrounding style.
2. **`scoped_runtime`** — add to `rust/analytics/src/lakehouse/runtime.rs`, taking the caller-built
   `Arc<ScopedMemoryPool>` and returning `Arc<RuntimeEnv>` from
   `RuntimeEnvBuilder::from_runtime_env(shared).with_memory_pool(scoped_pool).build()`.
3. **`LakehouseContext::with_runtime`** — struct-update clone in
   `rust/analytics/src/lakehouse/lakehouse_context.rs`, with a doc comment stating that the metadata
   cache and reader factory are shared.
4. **Audit record fields** — add `peak_memory_bytes`, `spilled_bytes`, `spill_count` to
   `QueryAuditRecord`; extend `ScanMetrics` and `aggregate_scan_metrics` in
   `rust/public/src/servers/query_audit.rs` to sum spills via `MetricsSet::spill_count()` /
   `spilled_bytes()` in the existing tree walk. Update the module doc comment.
5. **FlightSQL wiring** — in `execute_query`: construct the `ScopedMemoryPool` before `audit_state`
   and add it as `pool: Arc<ScopedMemoryPool>` on `QueryAuditState` from the start; build the scoped
   `RuntimeEnv`/context after, `map_err`-ing into `audit_state.emit("error", ...)` like every other
   setup stage; populate the three new fields in `emit()`; pass the scoped `LakehouseContext` to
   `make_session_context`; and emit the
   `query_peak_memory_bytes` `imetric!` from `emit()` itself — not the setup-phase timing-metrics
   block (`:451-462`), where the peak is still ~0. Since `emit()` is also called from the
   setup-error `map_err` closures and from the `Drop` impl for abandoned streams, those paths will
   contribute a (possibly ~0) sample too — intentional, so every audited query reports one point.
6. **Tests** — new `rust/analytics/tests/scoped_memory_pool_tests.rs` (auto-discovered;
   `analytics/Cargo.toml` has no `[[test]]` blocks) plus the `QueryAuditRecord` constructor/assertion
   updates in `rust/public/tests/query_audit_tests.rs`.
7. **Docs** — `mkdocs/docs/query-guide/query-audit-log.md` field table, a "most memory-hungry
   queries" example, and a Notes bullet on what the peak does and doesn't cover. CHANGELOG entry
   under `## Unreleased` → `**Analytics:**`.

## Files to Modify

| File | Change |
|------|--------|
| `rust/analytics/src/lakehouse/scoped_memory_pool.rs` | **new** — the wrapper |
| `rust/analytics/src/lakehouse/mod.rs` | declare the module |
| `rust/analytics/src/lakehouse/runtime.rs` | add `scoped_runtime` |
| `rust/analytics/src/lakehouse/lakehouse_context.rs` | add `with_runtime` |
| `rust/public/src/servers/query_audit.rs` | 3 new record fields; spill sums in `ScanMetrics` |
| `rust/public/src/servers/flight_sql_service_impl.rs` | scoped context in `execute_query`; `pool` on `QueryAuditState`; new `imetric!` |
| `rust/analytics/tests/scoped_memory_pool_tests.rs` | **new** — isolation + balance tests |
| `rust/public/tests/query_audit_tests.rs` | update the two record constructors + assertions |
| `mkdocs/docs/query-guide/query-audit-log.md` | field table, example, notes |
| `CHANGELOG.md` | Unreleased entry |

## Trade-offs

- **Tag `MemoryConsumer`s at registration** (the issue's original proposal): rejected — no extension
  point exists (finding 3), and ambient-context recovery leaks across DataFusion's internal task
  spawns. It would mostly work today and silently misattribute after a DataFusion bump.
- **Record each consumer's peak at `unregister`**: solves finding 1 but not attribution, and only
  pays off for a per-operator breakdown, which is a non-goal.
- **Poll `TrackConsumersPool::metrics()` and diff snapshots**: misses any consumer whose full
  lifetime fits between two polls (finding 1).
- **A separate memory *budget* per query**: unnecessary and harmful — it would fragment the global
  limit. Only a separate *wrapper* is needed; the inner pool, and therefore the budget, stays global.
- **Scope inside `make_session_context` instead of at `LakehouseContext`**: rejected — nested
  contexts (`perfetto_trace_execution_plan.rs:232`, `parse_block_table_function.rs:81`) and JIT
  materialization build their own session contexts from the captured `LakehouseContext`, so their
  memory would escape the scope.
- **Replace `TrackConsumersPool` with the scoped pool**: rejected — its top-consumer text in OOM
  errors is genuinely useful, and keeping it means the change is purely additive.
- **`reserved()` returning the scoped current**: rejected — it would change what DataFusion and the
  Arrow allocator shim see, for no gain; the scoped number is exposed as `current()` instead.

## Documentation

- `mkdocs/docs/query-guide/query-audit-log.md`:
  - Three rows in the **Fields** table: `peak_memory_bytes` (always; peak tracked DataFusion
    reservation for this query alone), `spilled_bytes` and `spill_count` (always; `0` unless a memory
    budget is configured).
  - A new example query under the existing "Slowest individual queries" section — same
    `jsonb_parse`/`jsonb_get`/`jsonb_as_i64` shape, `ORDER BY peak_memory_bytes DESC`, selecting
    `sql` for drill-down.
  - A **Notes** bullet: the peak is a per-query lower bound on process cost (list the untracked
    categories); it is a monotonic high-water mark, so it is valid on `error` and `incomplete`
    records too; and it is the signal to use when choosing a value for
    `MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB`, with `spilled_bytes`/`spill_count` staying `0` until
    that budget is set.
  - A second **Notes** bullet: `peak_memory_bytes` covers nested session contexts (Perfetto trace
    queries, JIT materialization) but `spilled_bytes`/`spill_count` only sum the outer plan tree, so
    a query can show `peak_memory_bytes > 0` with `spilled_bytes == 0` even once nested work has
    spilled. Also: a query that runs `materialize_partitions`/`regenerate_partitions` reports the
    merge's peak against the calling query, understated by the row-group-buffer caveat above.
- `CHANGELOG.md` under `## Unreleased`, new `**Analytics:**` bullet.
- No new page needed — the audit-log page is the natural home, and this is additive to a documented
  record.

## Testing Strategy

**Unit — `rust/analytics/tests/scoped_memory_pool_tests.rs`** (pure DataFusion, no data lake, no DB):

1. *Cross-query isolation (the regression test).* Build one shared
   `Arc<TrackConsumersPool<GreedyMemoryPool>>`, wrap it in two `ScopedMemoryPool`s, build a
   `RuntimeEnv` per scope, and run two queries concurrently via `tokio::join!` on plain
   `SessionContext`s over in-memory tables: a large `ORDER BY` over a generated table on one, and
   `SELECT 1` on the other. Assert the trivial query's `peak()` is `0` and the sort's is above a
   threshold. Keep the threshold loose (the sort's merge reservation alone contributes
   `sort_spill_reservation_bytes`, default 10 MB, per partition) and set
   `datafusion.execution.target_partitions` explicitly so the test isn't machine-dependent.
2. *Balance at quiescence.* After both futures complete and their streams/contexts are dropped,
   assert every scoped `current()` is `0` and the test's own `shared.reserved()` is `0`. This must
   use pools the test owns — asserting against a process-global pool would be flaky. Catches an
   unbalanced grow/shrink in the wrapper.
3. *Delegation.* With a `GreedyMemoryPool` of a small fixed size behind the wrapper: a `try_grow`
   past the limit returns `Err` **and** leaves `current()`/`peak()` unchanged; `memory_limit()`
   reports `Finite(n)` through the wrapper; a registered-then-dropped consumer leaves
   `shared.reserved() == 0` (proving `register`/`unregister` forwarding reached the inner pool).

**Unit — `rust/public/tests/query_audit_tests.rs`**: extend `full_record` and the
`omits_absent_optionals` record with the three new fields and assert they serialize (all three are
non-optional `u64`, so they must always be present); extend the `aggregate_scan_metrics` `FakeExec`
tests with, on two different child nodes, `MetricBuilder::new(&metrics).spill_count(0).add(n)` and
`.spilled_bytes(0).add(m)` for distinct non-zero `n`/`m` (the `0` argument is the partition index,
per the existing `.output_rows(0).add(rows)` pattern at `:61` — not a value), then assert the
tree-summed totals equal `n` and `m`. Non-zero values on non-root nodes are essential: they are what
makes a broken `sum_by_name`-based implementation (which returns `false`, hence `0`, for
`SpillCount`/`SpilledBytes`) actually fail the test instead of coincidentally matching.

**Integration**: start the local test env
(`python3 local_test_env/ai_scripts/start_services.py`), run a memory-hungry query
(`SELECT ... FROM log_entries ORDER BY msg LIMIT 100` over a real time range) alongside a trivial
one, then read back the audit rows with `micromegas-query`:

```sql
SELECT jsonb_as_i64(jsonb_get(jsonb_parse(msg), 'peak_memory_bytes')) AS peak,
       jsonb_as_string(jsonb_get(jsonb_parse(msg), 'sql')) AS sql
FROM log_entries WHERE target = 'flightsql_query_audit' ORDER BY peak DESC
```

Confirm the sort's `peak_memory_bytes > 0`, the trivial query's is `0`, and that a Perfetto-trace
query (`perfetto_trace_chunks`) reports non-zero — proving the nested-context inheritance.
Re-run once with `MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB` set low enough to force a spill and
confirm `spilled_bytes > 0` and that the OOM message still lists top consumers.

**Regression**: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`, and
`python3 ../build/rust_ci.py`. Confirm the existing audit fields and `imetric!`s are unchanged.

## Out of Scope

- **Daemon/maintenance-service merges** (`merge.rs:400`, `batch_update.rs`, `export_log_view.rs`,
  driven from `telemetry-maintenance-srv`) keep the shared pool. They have no audit record to report
  into, and the caveat that bites hardest there — the parquet writer's row-group buffers being
  invisible to the pool — would make the number misleading.
  Merges reached through the admin `materialize_partitions`/`regenerate_partitions` table functions
  (`materialize_partitions_table_function.rs`, `regenerate_partitions_table_function.rs`) are a
  different case: they run inside a FlightSQL query on the (now scoped) `LakehouseContext`, so they
  *are* in scope and will report a non-zero `peak_memory_bytes` for that query — understated by the
  same invisible-row-group-buffer caveat, since that's the dominant cost of a merge.
- **A per-operator breakdown.** Explicitly not an objective; `TrackConsumersPool`'s existing
  top-consumers text remains the tool for that, and now it is reachable per query via the scoped
  chain if that ever changes.
- **A normalized SQL fingerprint** on the audit record (still deferred, as in #1288).

