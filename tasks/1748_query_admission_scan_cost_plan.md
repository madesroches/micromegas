# Fair Per-Identity Query Queuing, Weighted by Estimated Scan Cost (#1748) Plan

## Overview

`flight-sql-srv` runs every query the moment it arrives, so one identity can fill the DataFusion
memory pool on its own. A shared service account behind dashboards that auto-refresh is the
usual case. This plan adds an admission scheduler that **never rejects**. Each identity gets its
own FIFO queue. A per-process budget caps how much work runs at once, and when capacity frees up the
scheduler admits the next query from the identity that currently has the least work in flight.
A refresh storm from one identity then waits in its own queue, while a lone notebook query from
anyone else goes straight to the front. A query's "cost" is the number of parquet bytes its
physical plan will scan. We compute that ourselves from the plan's scan nodes, because
DataFusion's own statistics are `Absent` for our scans (see Current State).

## Current State

- **Query path**: `FlightSqlServiceImpl::execute_query`
  (`rust/public/src/servers/flight_sql_service_impl.rs:672`) runs these steps in order: resolve
  attribution, deny-list check (`:798`), caller context, view-DDL intercept, scoped runtime,
  `make_session_context`, `ctx.sql` (logical planning, `:902`), optional limit,
  `df.create_physical_plan()` (`:930`), `execute_stream` (`:947`). The result is wrapped in a
  `CompletionTrackedStream` that owns `QueryAuditState` and emits the audit record when the
  stream drains, fails, or is dropped.
- **JIT materialization runs during physical planning.** `MaterializedView::scan`
  (`rust/analytics/src/lakehouse/materialized_view.rs:92`) calls `view.jit_update(...)` (`:110`) and only
  then fetches partitions and calls `make_partitioned_execution_plan`. `TableProvider::scan` runs
  inside `create_physical_plan`, so any gate placed after the physical plan exists comes *after*
  JIT work.
- **Scan nodes carry no size statistics.** `build_unordered_or_concatenated_plan` and
  `build_per_file_plan` (`rust/analytics/src/lakehouse/partitioned_execution_plan.rs:374`, `:428`)
  build a `DataSourceExec` over a `FileScanConfig` whose `PartitionedFile`s have
  `object_meta.size = partition.file_size`. They attach no statistics, except the leading
  ordering column's min/max on `Concatenated` scans (`attach_ordering_statistics`, `:124`).
  `FileScanConfig::statistics()` therefore reports `num_rows`/`total_byte_size` as `Absent`.
  Parquet row-group pruning happens later, inside the opener at execution time; nothing reads
  footers during planning.
- **DataFusion's projected byte size is useless for our schemas.** Even if we populated
  `total_byte_size`, `Statistics::calculate_total_byte_size` / `project_statistics`
  (datafusion-common 55.2 `stats.rs:433`) only rescale it when every projected column has a
  primitive width. Every view has string/dictionary/binary columns, so the projected value would
  just be the unprojected total, marked `Inexact`.
- **What is available at plan time:** each `PartitionedFile`'s `file_size`. The parsed
  `ParquetMetaData` may also already be in the process-wide `MetadataCache`
  (`rust/analytics/src/lakehouse/metadata_cache.rs:56`, `get` does not load), reachable through
  `LakehouseContext::metadata_cache()`. A cached footer gives exact per-column-chunk compressed
  sizes, so a projected estimate is possible without any I/O.
- **Actuals already recorded:** `aggregate_scan_metrics` (`rust/public/src/servers/query_audit.rs:33`)
  sums `bytes_scanned`, which `ReaderFactory` counts per `get_bytes`/`get_byte_ranges` request
  (`rust/analytics/src/lakehouse/reader_factory.rs:115`, `:144`). That is the same unit as the
  estimate below (compressed object-store bytes, footer reads excluded), so estimate vs. actual
  can be compared directly in the audit log. `peak_memory_bytes` comes from the per-query
  `ScopedMemoryPool`.
- **Opaque leaf nodes:** only `PerfettoTraceExecutionPlan`, the `process_spans` exec plan and
  `TaskLogExecPlan` do their work inside `execute`, so their scan cost cannot be seen at plan
  time. `parse_block` and the `list_*` system-table UDTFs do their work at plan time, in
  `scan`/`call`, and return a `DataSourceExec(MemorySourceConfig)` that is already materialized.
  Static CSV/JSON `ListingTable`s produce non-parquet `FileScanConfig`s.

## Design

### 0. Scan statistics: not a prerequisite (TL;DR)

Populating DataFusion statistics on our scans would not give the admission estimate what it needs:
the projected `total_byte_size` stays the unprojected total for any schema with a string column
(see Current State). The estimator therefore walks the plan itself (§1) either way. Filling
`num_rows`/`total_byte_size` from partition metadata as `Inexact` is still worth doing on its own,
because without statistics `JoinSelection` always builds the hash table on whichever side is written
first. That is a separate follow-up; plan-time footer statistics belong with #1234.

To keep #1234 cheap, the estimator does all its footer lookups through one helper
(`cached_footer(path)`). If #1234 swaps `MetadataCache` for DataFusion's `FileMetadataCache`,
only that helper changes.

### 1. Scan-cost estimator (`micromegas_analytics::lakehouse::scan_cost`)

```rust
pub enum CostPrecision { Footer, FileSizeShare, Opaque }

pub struct ScanCostEstimate {
    pub bytes: u64,                 // estimated compressed bytes the scans will request
    pub files: u64,
    pub footer_files: u64,          // files estimated from a cached footer
    pub opaque_nodes: u64,          // leaves we could not see into
    pub precision: CostPrecision,   // worst precision among contributors; the fold starts from
                                    // `Footer` (the best), so no contributors reports `footer`
}

pub async fn estimate_scan_cost(
    plan: &Arc<dyn ExecutionPlan>,
    metadata_cache: &MetadataCache,
    opaque_leaf_cost: u64,
) -> ScanCostEstimate
```

The estimator walks the plan tree:

- **`DataSourceExec` whose data source is a `MemorySourceConfig`:** 0, the data is already
  materialized.
- **`DataSourceExec` whose data source is a `FileScanConfig`:** `file_column_projection_indices()`
  is deprecated and panics when an expression projection is pushed into the scan, so do not use
  it. Get the parquet source with `DataSourceExec::downcast_to_file_source::<ParquetSource>()`
  and take the projected columns from `file_source.projection().map(|p| p.column_indices())`,
  filtered to indices below the file schema's field count. `None` means all columns. A
  `FileScanConfig` whose source is not `ParquetSource` (static CSV/JSON tables) is costed at the
  full `file_size`, with no footer or column-share path; its precision is `FileSizeShare`. For parquet, collect the
  `PartitionedFile`s of all `file_groups` and **dedupe by `object_meta.location`** before
  summing: with `repartition_file_scans` (the default in user sessions) the optimizer splits one
  file into several byte-range `PartitionedFile`s, and each file must be charged once. Then for
  each distinct file:
  - *Footer cached* (`cached_footer(path)`, which looks up `pf.object_meta.location.as_ref()`,
    the object_store `Path` string the cache is keyed by, not the raw `Partition::file_path`):
    map the projected top-level roots to parquet leaf column chunks with
    `SchemaDescriptor::get_column_root_idx`, and sum `compressed_size()` of those chunks over all
    row groups. This is an upper bound because row-group/page pruning only reduces it. An empty
    projection, as in `count(*)`, costs 0 here.
  - *Not cached*: use `file_size × projected_leaf_columns / total_leaf_columns`, where the leaf
    counts come from the arrow schema's leaf count (or the root count when no footer is
    available). Precision is `FileSizeShare`. The plan **does not load footers** (see Trade-offs).
- **`EmptyExec` / `PlaceholderRowExec`:** 0.
- **Any other leaf, i.e. no children (opaque table functions, ...):** add
  `opaque_leaf_cost` and mark the precision `Opaque`. Letting these leaves report their own
  estimate is tracked in #1751.
- **Inner nodes:** sum the children. Joins, aggregates and sorts add nothing to the cost. Their
  memory is not predictable from the plan, and the pool reservation stays the authority for it.

The estimator never fails. A downcast that does not match counts as an opaque leaf. The function
is `async` only because `MetadataCache::get` is.

### 2. Admission scheduler (`micromegas_public::servers::query_admission`)

The scheduler is generic over cost and identity. It knows nothing about DataFusion, so it can be
unit-tested in isolation.

```rust
pub struct AdmissionConfig {
    pub budget: u64,          // per-process cost units allowed in flight
    pub min_cost: u64,        // floor per query so 0-byte queries still occupy a slot
}

pub struct AdmissionScheduler { state: Mutex<State>, config: AdmissionConfig }

struct State {
    in_flight_total: u64,
    queued_total: u64,       // waiters across all identities
    next_arrival: u64,       // counter stamped on each Waiter
    per_identity: HashMap<Arc<str>, IdentityState>,  // removed when idle and empty
}
struct IdentityState { in_flight: u64, queue: VecDeque<Waiter> }
struct Waiter { cost: u64, arrival: u64, grant: oneshot::Sender<()> }

impl AdmissionScheduler {
    pub async fn admit(self: &Arc<Self>, identity: Arc<str>, cost: u64) -> AdmissionPermit;
}
pub struct AdmissionPermit { /* identity, charged cost, Arc<scheduler>; Drop releases */ }
```

Rules:

- **Charged cost** = `clamp(estimate, min_cost, budget)`. Clamping to `budget` lets a query
  bigger than the whole budget still run, alone, instead of waiting forever. That is what
  "never reject" requires.
- **Fast path:** if no identity has a waiter (`queued_total == 0`) and
  `in_flight_total + cost <= budget`, admit at once with zero wait. Otherwise a small query
  from an idle identity would pass a blocked head.
- **Otherwise** enqueue a `Waiter` that holds a `oneshot::Sender`, then wait.
- **Dispatch** runs on every enqueue and every permit release:
  1. Among identities with a non-empty queue, pick the one with the smallest `in_flight`. Break
     ties by the smallest head-of-queue `arrival`.
  2. If its head fits (`in_flight_total + cost <= budget`), grant it and repeat.
  3. If it does not fit, **stop**. Do not let a smaller query from another identity jump past
     it. Without this rule, a large query could starve behind a steady stream of small ones.
     Charged cost is at most `budget`, so the head always fits once enough in-flight work drains.

  Picking the identity with the least in-flight work is max-min fairness. Light users see almost
  no wait, and a flood from one identity is served at the pace its own share allows.
- **Cancellation:** tonic drops the `do_get` future when the client goes away, and the `Waiter`
  is then removed from its queue by a drop guard around the wait. If a grant raced the drop
  (the sender fired but the permit was never taken), the guard releases the granted cost and
  re-runs dispatch. This guard is the one subtle piece of the scheduler and has its own tests.
  The wait's drop guard at the `execute_query` call site holds `&mut QueryAuditState` (or owns
  it across the await and hands it back on grant) and records the wait-start `Instant`. When the
  future is dropped before a permit is granted, it sets `queue_wait_ms` and then calls
  `emit("incomplete")`, the same path `CompletionTrackedStream::drop` uses.
- **Permit lifetime:** the permit is held until the result stream completes, fails, or is
  dropped. That is the same lifetime the audit record already tracks.

**Disabled mode:** `FlightSqlServiceImpl` holds an `Option<Arc<AdmissionScheduler>>`, where
`None` (no budget configured) means disabled: no `admit` call, permit `None`, and
`admission_cost`/`queue_wait_ms` absent. The estimate is still computed and recorded (see §4), which allows the calibration
step to run in production before anything is queued.

### 3. Wiring in `execute_query`

The gate goes right after `create_physical_plan` and before `execute_stream`:

```
create_physical_plan ─► estimate_scan_cost(plan, self.lakehouse.metadata_cache(), opaque_leaf_cost)
                     ─► scheduler.admit(identity_key, estimate.bytes).await   // may wait
                     ─► execute_stream ─► CompletionTrackedStream (owns permit via audit_state)
```

- **The gate must precede `execute_stream`, not just the first poll.** For a plan with more than
  one output partition, `execute_stream` wraps it in `CoalescePartitionsExec`, whose `execute`
  spawns one tokio task per input partition right away (datafusion-physical-plan 55.2
  `coalesce_partitions.rs:241`). Scanning therefore starts as soon as `execute_stream` returns,
  before the client pulls a single batch.
- **Identity key:** `attr.service_account` when present, otherwise `attr.user_id`. An OIDC user
  maps to `user_id`. A non-delegating key has `user_id` = the key's subject and no
  `service_account`. A delegating key has `service_account` set, and all the users it acts for
  share that account's queue.
- **Permit ownership:** the permit is stored on `QueryAuditState` as an
  `Option<AdmissionPermit>`. The audit state already travels into `CompletionTrackedStream` and
  is dropped on every terminal path, including the early `audit_state.fail(...)` returns, so the
  permit is released on every path with no new plumbing.
- **Timing:** `execution_begin`/`execution_begin_instant` are taken before
  `create_physical_plan`, and `execution_ms`, `setup_ms` and the
  `query_execution_duration`/`query_setup_duration` metrics are computed after `execute_stream`,
  so a gate between them would fold the wait into all four. The metrics are `"ticks"` from
  `now()` differences, while `execution_ms`/`setup_ms` are milliseconds from `Instant`. Around
  `admit`, capture both the `now()` tick delta and the `Instant` delta. Subtract the tick delta
  from the two tick metrics and the ms delta from `execution_ms` and `setup_ms`; `queue_wait_ms`
  is the ms delta. Existing latency dashboards keep their meaning. `total_ms` and `query_duration_total` include the wait.
- **View DDL and deny-listed queries** never reach the gate.

### 4. Observability

Audit record (`QueryAuditRecord`): append these JSON fields **last**:
`estimated_scan_bytes`, `estimate_precision` (`"footer" | "file_size_share" | "opaque"`),
`admission_cost` (the charged cost, absent when admission is disabled), and `queue_wait_ms`.
All four are `Option<...>` with `skip_serializing_if`, absent when the estimate or gate did not
run (denied, view-DDL, planning errors). The struct literals in `query_audit_tests.rs` need
updating.
Per-identity attribution of queuing lives here, because the audit record has no cardinality
limit.

Metrics (`imetric!`, with no per-identity tags, because identities are unbounded cardinality):
`admission_queue_wait` (ticks, from the tick delta captured around `admit`), `admission_queued` (count, only for queries that missed the fast
path), `admission_in_flight_cost` and `admission_queued_queries` (gauges sampled on dispatch).

### 5. Configuration

The settings follow the existing env-var convention for `flight-sql-srv` and the monolith:

- `MICROMEGAS_ADMISSION_BUDGET_MB`: per-process in-flight scan-cost budget. Unset means disabled
  (estimate-only).
- `MICROMEGAS_ADMISSION_MIN_COST_MB` (default 8).
- `MICROMEGAS_ADMISSION_OPAQUE_LEAF_COST_MB` (default 64).

`MICROMEGAS_ADMISSION_BUDGET_MB` is parsed like `parse_max_temp_directory_mb`
(`rust/analytics/src/lakehouse/runtime.rs`, `Result<Option<u64>>`): unset is `None` (disabled)
and an unparseable value is an error that fails startup in `build_and_serve`, so a typo such as
`64MB` cannot silently run estimate-only. MB converts to bytes with `checked_mul`, and overflow
also fails startup. The two cost knobs are read with `resolve_u64_env`
(`rust/public/src/settings.rs`) with their defaults. `build_and_serve` fails fast at startup with a clear error when
`budget == 0` or `min_cost > budget`, because `clamp(estimate, min_cost, budget)` panics when
`min_cost > budget`. The check is a pure function over the parsed values.

The budget is in **scan bytes**, not memory bytes. Operators size it from the calibration query
in the docs, which compares `estimated_scan_bytes` with `peak_memory_bytes` and with the pool
size. A pool-derived default is a possible follow-up once calibration data exists.

## Implementation Steps

**Phase 1: estimate only (no behavior change)**

1. Add `rust/analytics/src/lakehouse/scan_cost.rs` (§1) and register it in `lakehouse/mod.rs`.
2. Add the `estimated_scan_bytes` / `estimate_precision` fields to `QueryAuditState` and
   `QueryAuditRecord` (`query_audit.rs`), appended last. `QueryAuditState`, its `emit()` mapping
   to `QueryAuditRecord`, and its initializer live in `flight_sql_service_impl.rs`; only
   `QueryAuditRecord` is in `query_audit.rs`.
3. In `FlightSqlServerBuilder::build_and_serve` (`flight_sql_server.rs`, the only production
   construction site, shared by flight-sql-srv and the monolith), read the env config and pass it to
   `FlightSqlServiceImpl::new`. In `execute_query`, call the estimator after
   `create_physical_plan`, store the result on `audit_state`, and use the opaque-leaf default
   from that config.
4. Tests (see Testing Strategy), docs, and a CHANGELOG entry under a newly added
   `## Unreleased` heading, with a **Minor breaking change** clause (new `QueryAuditRecord`
   fields, new `FlightSqlServiceImpl::new` parameter).

**Phase 2: scheduler**

5. Add `rust/public/src/servers/query_admission.rs` (§2) and register it in `servers/mod.rs`.
6. Build `AdmissionScheduler` from env in `build_and_serve` (`flight_sql_server.rs`). Hand it
   to `FlightSqlServiceImpl` next to the other shared services.
7. Gate `execute_query` (§3). Add `admission_cost`, `queue_wait_ms` and the permit to the audit
   state, and emit the metrics (§4).
8. Tests, docs, and the CHANGELOG entry (with its **Minor breaking change** clause).

Phase 1 can ship and run alone, which gives the measurement the issue asks for before Phase 2 is
enabled anywhere.

## Files to Modify

- `rust/analytics/src/lakehouse/scan_cost.rs` (new), `rust/analytics/src/lakehouse/mod.rs`
- `rust/analytics/tests/scan_cost_tests.rs` (new)
- `rust/public/src/servers/query_admission.rs` (new), `rust/public/src/servers/mod.rs`
- `rust/public/tests/query_admission_tests.rs` (new)
- `rust/public/src/servers/flight_sql_service_impl.rs`: estimate, gate, permit
- `rust/public/src/servers/query_audit.rs`: new `QueryAuditRecord` fields
- `rust/public/tests/query_audit_tests.rs`: serialization of new fields
- `rust/public/tests/read_policy_threading_tests.rs`: the two `FlightSqlServiceImpl::new` calls
  take the new arguments; `start_server` gains a scheduler parameter for the cancel test
- `rust/public/src/servers/flight_sql_server.rs`: config and scheduler construction from env
- `mkdocs/docs/query-guide/query-audit-log.md`: the four fields in the `## Fields` table
- `mkdocs/docs/admin/flight-sql.md` (env table plus an Admission section),
  `mkdocs/docs/admin/monolith.md` (env table)
- `CHANGELOG.md`

## Trade-offs

- **Footers: cached only vs. loaded at plan time.** Loading uncached footers during planning
  would make every estimate `Footer`-precise and would warm the cache that execution needs
  anyway. It would also put object-store I/O, proportional to the partition count, in front of
  the gate, for queries that may then sit in the queue. Dashboards repeat the same partitions,
  so the cache is warm for the traffic that matters, and the `FileSizeShare` fallback is
  adequate for a queue-ordering signal.
- **Gate after physical planning vs. before planning.** Before planning there is no cost to
  weigh, only a count. After planning, JIT materialization has already run, unqueued. We accept
  that. JIT is bounded per partition, already serialized per view instance, and paid once per
  range, whereas repeated dashboard scans are the dominant load. Estimating and admitting JIT work
  is tracked in #1750.
- **Least-in-flight with no skipping vs. deficit round robin / skip-to-fit.** Skip-to-fit uses
  the budget better but starves large queries. DRR adds a quantum to tune. Least-in-flight plus
  head-of-line blocking is starvation-free, needs no tuning, and is easy to explain.
- **Scan bytes as the cost unit.** Scans are what the pool under-counts (decode buffers are
  untracked), and the estimate is checkable against `bytes_scanned` per query. Aggregation and
  join memory is not predictable from the plan in any case.

## Decisions

- No rejection path: queries wait for their turn; there is no bounded wait and no queue-depth
  cap (user call).
- Scan statistics are not a prerequisite. Populating them from partition metadata (`Inexact`) is
  its own follow-up issue, and plan-time footer statistics stay with #1234.
- Queue key excludes `client`: it is client-asserted and would let a caller multiply its share.

## Documentation

- `mkdocs/docs/query-guide/query-audit-log.md`: the four new fields in the `## Fields` table. Add a calibration query
  comparing `estimated_scan_bytes`, `bytes_scanned` and `peak_memory_bytes` grouped by
  `sql_hash`, and a "who is waiting" query on `queue_wait_ms` grouped by user/service account.
- `mkdocs/docs/admin/flight-sql.md` and `mkdocs/docs/admin/monolith.md`: the three
  `MICROMEGAS_ADMISSION_*` variables, the disabled-by-default behavior, a note that the budget and fairness apply per replica (sized against each replica's own memory pool), and guidance for sizing the budget from the calibration query.
- `CHANGELOG.md` (new `## Unreleased` heading): estimate fields, and admission scheduler (off by default), with a **Minor breaking change** clause.

## Testing Strategy

All coverage is no-DB unit tests.

- **`scan_cost_tests.rs`:** write a small parquet file to an in-memory object store, load its
  `ParquetMetaData` into a `MetadataCache`, then build plans with
  `make_partitioned_execution_plan` over hand-built `Partition`s.
  - Full projection, cached footer: equals the sum of all column chunks' compressed sizes.
  - Narrow projection: only the projected chunks are counted.
  - Empty projection (`count(*)`-shaped): 0.
  - Uncached file: `file_size × projected/total`, precision `FileSizeShare`.
  - Mix of cached and uncached files: sums correctly, and the worst precision wins.
  - All-empty partitions (`EmptyExec`): 0, precision `Footer` (the fold's starting value).
  - Join of two scans: both are summed.
  - Opaque leaf (a stub `ExecutionPlan` with no children): adds `opaque_leaf_cost`, precision
    `Opaque`.
  - `PerFile` and `Concatenated` scan shapes give the same estimate as `Unordered` over the same
    files. The fixtures must set `sort_order` (so `PerFile` certifies instead of degrading to
    `Unordered`) and non-overlapping bounds (so `Concatenated` doesn't error), reusing the
    helpers in `rust/analytics/tests/per_file_scan_ordering_tests.rs`.
  - Run the plan through the physical optimizer with `target_partitions > 1` and a file at least
    `repartition_file_min_size`: the file is split into byte ranges but counted once.
- **`query_admission_tests.rs`** (tokio, paused time where useful):
  - Fast path: no wait while under budget.
  - Over budget: the waiter is admitted when a permit drops.
  - Fairness: identity A queues 10, then B queues 1. B's query is admitted next, ahead of A's
    remaining queue.
  - Least-in-flight ordering across three identities.
  - Head-of-line: a large head blocks smaller queries from other identities until it fits, so it
    is not starved.
  - Fast path with a blocked head: a fresh identity's small query arrives while a large head is
    blocked, and it waits behind the head.
  - Cost clamp: a query whose cost exceeds the budget runs alone.
  - `min_cost` floor applied.
  - Cancellation while queued removes the waiter, and the queues behind it advance.
  - Cancellation racing a grant releases the granted cost, so the budget does not leak.
  - Idle identities are removed from the map.
- **Config validation** (pure function over the parsed values): `budget == 0` and
  `min_cost > budget` are errors; `min_cost <= budget` passes. Budget parsing: unset is `None`,
  unparseable (`64MB`) is an error, and a value whose byte conversion overflows is an error.
- **Cancel while queued** (no-DB gRPC harness `start_server` in
  `rust/public/tests/read_policy_threading_tests.rs`, which gains an
  `Option<Arc<AdmissionScheduler>>` parameter; `#[serial]`, with `init_in_memory_tracing` /
  `InMemorySink` capturing audit lines as in `auth_observability_tests.rs`). Budget = `min_cost`.
  The held query must outlast the test: `SELECT * FROM generate_series(1, 100000000)`, read for
  one batch and then left undrained. A `SELECT 1` would finish sending within the HTTP/2
  flow-control window and release its permit at once, so nothing would queue. Start a second
  `do_get`, wait until `AdmissionScheduler::queued_count()` (the accessor behind the
  `admission_queued_queries` gauge) reports 1, then cancel it. Assert exactly one `incomplete` audit record with
  `queue_wait_ms` set. Drop the held stream and assert a third query is admitted, so no budget
  leaks.
- **`query_audit_tests.rs`:** the new fields serialize after `sql_hash`, asserted on byte
  offsets in the raw JSON string (offset of `"sql_hash"` < offset of `"estimated_scan_bytes"`),
  not on a parsed `serde_json::Value`; `admission_cost` is absent when admission is disabled.

## Manual Verification

The end-to-end wiring (env → scheduler → `execute_query` → permit release on stream drop) runs
across the real gRPC handler. If it broke, queries would visibly hang or never queue the next
time anyone ran the steps below.

1. `python3 local_test_env/ai_scripts/start_services.py` with `MICROMEGAS_ADMISSION_BUDGET_MB=64`.
2. From Python, fire 20 concurrent `SELECT * FROM log_entries` with one identity's attribution
   headers, and 1 concurrent query with a second identity. Under `--disable-auth`, identities
   are set through the `x-user-id` (and `x-user-email`) headers (`start_services.py` already
   sets `MICROMEGAS_TELEMETRY_URL`).
3. `micromegas-query "SELECT msg FROM log_entries WHERE target = 'flightsql_query_audit' ORDER BY time DESC LIMIT 25" --begin 5m`.
   Expected: the second identity waits for at most about one release, far less than the first
   identity's tail, the first identity's waits grow down its queue, and every query has status
   `ok`.
4. Cancel a client mid-wait (Ctrl-C). Expected: the remaining queued queries complete (no leaked
   budget). The cancelled query emits one `incomplete` audit record with a non-zero `queue_wait_ms`.

## Open Questions

1. **Per-identity weights:** does a shared dashboard service account need a larger share (a
   weight in the least-in-flight comparison), or is equal share per identity the intended QoS?
