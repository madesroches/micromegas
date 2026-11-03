# Fair Per-Identity Query Queuing, Weighted by Estimated Scan Cost (#1748) Plan

## Overview

`flight-sql-srv` runs every query the moment it arrives, so one identity can fill the DataFusion
memory pool on its own. A shared service account behind dashboards that auto-refresh is the
usual case. This plan adds an admission scheduler that **never rejects**. Each identity gets its
own FIFO queue. A global budget caps how much work runs at once, and when capacity frees up the
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
  (`rust/analytics/src/lakehouse/materialized_view.rs:91`) calls `view.jit_update(...)` and only
  then fetches partitions and calls `make_partitioned_execution_plan`. `TableProvider::scan` runs
  inside `create_physical_plan`, so any gate placed after the physical plan exists comes *after*
  JIT work.
- **Scan nodes carry no size statistics.** `build_unordered_or_concatenated_plan` and
  `build_per_file_plan` (`rust/analytics/src/lakehouse/partitioned_execution_plan.rs:366`, `:418`)
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
- **Opaque leaf nodes:** `PerfettoTraceExecutionPlan`, `process_spans`, `TaskLogExecPlan`,
  `parse_block` and the system-table UDTFs build their work inside `execute`. Their scan cost
  cannot be seen at plan time.

## Design

### 0. Should scan statistics be fixed first? (research)

Our `FileScanConfig`s carry no `num_rows`/`total_byte_size`, so every DataFusion rule that reads
statistics sees `Absent`. In DataFusion 55.2 the affected rules are:

- **`JoinSelection`** (`join_selection.rs:88`, `:118`). `should_swap_join_order` compares
  `total_byte_size`, then `num_rows`, and returns `false` when either side is `Absent`. So the
  hash-join build side is always whichever side the user wrote on the left. Writing
  `log_entries JOIN processes` builds the hash table over the log entries.
  `supports_collect_by_thresholds` also returns `false`, so a small dimension side is never
  chosen for `CollectLeft`.
- **`AggregateStatistics`**: answers `count(*)` / `min` / `max` from `Exact` statistics. It is
  inert today, and it should stay inert (see S1 below).
- Filter-selectivity and aggregate cardinality estimates propagate whatever the leaves report.

There are two ways to populate the statistics:

- **S1: from partition metadata, no I/O.** `Partition::num_rows` comes from the footer at write
  time (`write_partition.rs:820`), so it is exact per file, and `file_size` is known. Set
  per-file and per-scan `num_rows` = Σ `num_rows` and `total_byte_size` = Σ `file_size`, both
  **`Inexact`**. `Inexact` is enough for `JoinSelection` (`get_value()` accepts it), and it keeps
  `AggregateStatistics` from answering a query out of metadata. That rule could silently diverge
  from the scanned rows if a partition row were ever wrong, which would be a correctness risk
  for little gain. The cost is a dozen lines in `partitioned_execution_plan.rs`, and it may
  change join plans, which needs before/after `EXPLAIN` on representative joins.
- **S2: full column statistics from footers at plan time** (min/max/null counts, per-column
  `byte_size`). This needs footer reads during planning, the same I/O this plan deliberately
  keeps out of the pre-gate path. It belongs with #1234: adopting DataFusion's
  `CachedParquetFileReaderFactory` / `FileMetadataCache` is where plan-time footer access gets
  a first-class, cached home.

**Conclusion: don't block #1748 on either.** Neither gives the admission estimate what it needs.
DataFusion's projection keeps the *unprojected* `total_byte_size` whenever a projected column has
no primitive width (`Statistics::calculate_total_byte_size`, datafusion-common `stats.rs:433`),
and it ignores per-column `byte_size` even when present. Every view has string or dictionary
columns, so `SELECT time FROM log_entries` would still be costed as the whole file. The projected
estimate needs the estimator's own walk (§1) in every case. S1 is still worth doing on its own
merits (hash-join build side) as a separate small change, tracked as its own issue. It is
independent of this plan and can land before or after it.

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
    pub precision: CostPrecision,   // worst precision among contributors
}

pub async fn estimate_scan_cost(
    plan: &Arc<dyn ExecutionPlan>,
    metadata_cache: &MetadataCache,
    opaque_leaf_cost: u64,
) -> ScanCostEstimate
```

The estimator walks the plan tree:

- **`DataSourceExec` whose data source is a `FileScanConfig`:** take the projected file-column
  indices from `FileScanConfig::file_column_projection_indices()`, where `None` means all
  columns. Then for each `PartitionedFile` in `file_groups`:
  - *Footer cached* (`metadata_cache.get(path)`): sum `compressed_size()` of the projected column
    chunks over all row groups. This is an upper bound because row-group/page pruning only
    reduces it. An empty projection, as in `count(*)`, costs 0 here.
  - *Not cached*: use `file_size × projected_leaf_columns / total_leaf_columns`, where the column
    counts come from the table schema. Precision is `FileSizeShare`. The plan **does not load
    footers**: that would add object-store I/O before the gate, and a query waiting in the queue
    should not have touched storage yet.
- **`EmptyExec` / `PlaceholderRowExec`:** 0.
- **Any other leaf, i.e. no children (opaque table functions, `MemorySourceConfig`, ...):** add
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
    pub budget: u64,          // global cost units allowed in flight
    pub min_cost: u64,        // floor per query so 0-byte queries still occupy a slot
}

pub struct AdmissionScheduler { state: Mutex<State>, config: AdmissionConfig }

struct State {
    in_flight_total: u64,
    per_identity: HashMap<Arc<str>, IdentityState>,  // removed when idle and empty
}
struct IdentityState { in_flight: u64, queue: VecDeque<Waiter> }

impl AdmissionScheduler {
    pub async fn admit(self: &Arc<Self>, identity: Arc<str>, cost: u64) -> AdmissionPermit;
}
pub struct AdmissionPermit { /* identity, charged cost, Arc<scheduler>; Drop releases */ }
```

Rules:

- **Charged cost** = `clamp(estimate, min_cost, budget)`. Clamping to `budget` lets a query
  bigger than the whole budget still run, alone, instead of waiting forever. That is what
  "never reject" requires.
- **Fast path:** if the identity's queue is empty and `in_flight_total + cost <= budget`, admit
  at once with zero wait.
- **Otherwise** enqueue a `Waiter` that holds a `oneshot::Sender`, then wait.
- **Dispatch** runs on every enqueue and every permit release:
  1. Among identities with a non-empty queue, pick the one with the smallest `in_flight`. Break
     ties by the oldest head-of-queue arrival.
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
- **Permit lifetime:** the permit is held until the result stream completes, fails, or is
  dropped. That is the same lifetime the audit record already tracks.

**Disabled mode:** when no budget is configured, `admit` returns immediately and does not touch
the mutex. The estimate is still computed and recorded (see §4), which allows the calibration
step to run in production before anything is queued.

### 3. Wiring in `execute_query`

The gate goes right after `create_physical_plan` and before `execute_stream`:

```
create_physical_plan ─► estimate_scan_cost(plan, lakehouse.metadata_cache(), opaque_leaf_cost)
                     ─► scheduler.admit(identity_key, estimate.bytes).await   // may wait
                     ─► execute_stream ─► CompletionTrackedStream (owns permit via audit_state)
```

- **The gate must precede `execute_stream`, not just the first poll.** For a plan with more than
  one output partition, `execute_stream` wraps it in `CoalescePartitionsExec`, whose `execute`
  spawns one tokio task per input partition right away (datafusion-physical-plan 55.2
  `coalesce_partitions.rs:241`). Scanning therefore starts as soon as `execute_stream` returns,
  before the client pulls a single batch.
- **Identity key:** `attr.service_account` when present, otherwise `attr.user_id`. This is the
  same identity the deny list already matches on.
- **Permit ownership:** the permit is stored on `QueryAuditState` as an
  `Option<AdmissionPermit>`. The audit state already travels into `CompletionTrackedStream` and
  is dropped on every terminal path, including the early `audit_state.fail(...)` returns, so the
  permit is released on every path with no new plumbing.
- **Timing:** time spent waiting is measured separately as `queue_wait_ms`. It is not folded into
  `execution_ms`, so existing latency dashboards keep their meaning.
- **View DDL and deny-listed queries** never reach the gate.

### 4. Observability

Audit record (`QueryAuditRecord`): append these JSON fields **last**:
`estimated_scan_bytes`, `estimate_precision` (`"footer" | "file_size_share" | "opaque"`),
`admission_cost` (the charged cost, absent when admission is disabled), and `queue_wait_ms`.
Per-identity attribution of queuing lives here, because the audit record has no cardinality
limit.

Metrics (`imetric!`, with no per-identity tags, because identities are unbounded cardinality):
`admission_queue_wait` (ms), `admission_queued` (count, only for queries that missed the fast
path), `admission_in_flight_cost` and `admission_queued_queries` (gauges sampled on dispatch).

### 5. Configuration

The settings follow the existing env-var convention for `flight-sql-srv` and the monolith:

- `MICROMEGAS_ADMISSION_BUDGET_MB`: global in-flight scan-cost budget. Unset means disabled
  (estimate-only).
- `MICROMEGAS_ADMISSION_MIN_COST_MB` (default 8).
- `MICROMEGAS_ADMISSION_OPAQUE_LEAF_COST_MB` (default 64).

The budget is in **scan bytes**, not memory bytes. Operators size it from the calibration query
in the docs, which compares `estimated_scan_bytes` with `peak_memory_bytes` and with the pool
size. The pool stays the hard limit, so a wrong estimate only changes queue order and pacing. It
can never cause a rejection or an OOM that would not have happened anyway.

## Implementation Steps

**Phase 1: estimate only (no behavior change)**

1. Add `rust/analytics/src/lakehouse/scan_cost.rs` (§1) and register it in `lakehouse/mod.rs`.
2. Add the `estimated_scan_bytes` / `estimate_precision` fields to `QueryAuditState` and
   `QueryAuditRecord` (`query_audit.rs`), appended last.
3. In `execute_query`, call the estimator after `create_physical_plan`, store the result on
   `audit_state`, and read the opaque-leaf default from config.
4. Tests (see Testing Strategy), docs, and a CHANGELOG entry.

**Phase 2: scheduler**

5. Add `rust/public/src/servers/query_admission.rs` (§2) and register it in `servers/mod.rs`.
6. Build `AdmissionScheduler` from env in `flight_sql_server.rs` / the monolith's analytics
   role. Hand it to `FlightSqlServiceImpl` next to the other shared services.
7. Gate `execute_query` (§3). Add `admission_cost`, `queue_wait_ms` and the permit to the audit
   state, and emit the metrics (§4).
8. Tests, docs, and a CHANGELOG entry.

Phase 1 can ship and run alone, which gives the measurement the issue asks for before Phase 2 is
enabled anywhere.

## Files to Modify

- `rust/analytics/src/lakehouse/scan_cost.rs` (new), `rust/analytics/src/lakehouse/mod.rs`
- `rust/analytics/tests/scan_cost_tests.rs` (new)
- `rust/public/src/servers/query_admission.rs` (new), `rust/public/src/servers/mod.rs`
- `rust/public/tests/query_admission_tests.rs` (new)
- `rust/public/src/servers/flight_sql_service_impl.rs`: estimate, gate, permit
- `rust/public/src/servers/query_audit.rs`: new fields
- `rust/public/tests/query_audit_tests.rs`: serialization of new fields
- `rust/public/src/servers/flight_sql_server.rs`, monolith analytics-role setup: scheduler
  construction from env
- `mkdocs/docs/query-guide/query-audit-log.md`, admin/deployment docs for the env vars
- `CHANGELOG.md`

## Trade-offs

- **Our own plan walk vs. reading DataFusion `Statistics`.** Reading `partition_statistics(None)`
  would lose the projection for every schema with a string column. See §0 for the full analysis,
  and for why populating the statistics is a separate change.
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
- **Never reject.** This matches the requirement that callers only ever see latency. The
  deny-list `ResourceExhausted` path (#1488) is unchanged and is still the operator's reactive
  tool. Queue depth is unbounded, but each waiter is a small struct held by a live client
  request, so memory grows with connected clients, not with data.
- **Scan bytes as the cost unit.** Scans are what the pool under-counts (decode buffers are
  untracked), and the estimate is checkable against `bytes_scanned` per query. Aggregation and
  join memory is not predictable from the plan in any case.

## Decisions

- No rejection path: queries wait for their turn; there is no bounded wait and no queue-depth
  cap (user call).
- Scan statistics are not a prerequisite. Populating them from partition metadata (`Inexact`) is
  its own follow-up issue, and plan-time footer statistics stay with #1234 (§0).

## Documentation

- `mkdocs/docs/query-guide/query-audit-log.md`: the four new fields. Add a calibration query
  comparing `estimated_scan_bytes`, `bytes_scanned` and `peak_memory_bytes` grouped by
  `sql_hash`, and a "who is waiting" query on `queue_wait_ms` grouped by user/service account.
- Admin/deployment docs: the three `MICROMEGAS_ADMISSION_*` variables, the disabled-by-default
  behavior, and guidance for sizing the budget from the calibration query.
- `CHANGELOG.md` (Unreleased): estimate fields, and admission scheduler (off by default).

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
  - All-empty partitions (`EmptyExec`): 0.
  - Join of two scans: both are summed.
  - Opaque leaf (a stub `ExecutionPlan` with no children): adds `opaque_leaf_cost`, precision
    `Opaque`.
  - `PerFile` and `Concatenated` scan shapes give the same estimate as `Unordered` over the same
    files.
- **`query_admission_tests.rs`** (tokio, paused time where useful):
  - Fast path: no wait while under budget.
  - Over budget: the waiter is admitted when a permit drops.
  - Fairness: identity A queues 10, then B queues 1. B's query is admitted next, ahead of A's
    remaining queue.
  - Least-in-flight ordering across three identities.
  - Head-of-line: a large head blocks smaller queries from other identities until it fits, so it
    is not starved.
  - Cost clamp: a query whose cost exceeds the budget runs alone.
  - `min_cost` floor applied.
  - Cancellation while queued removes the waiter, and the queues behind it advance.
  - Cancellation racing a grant releases the granted cost, so the budget does not leak. This is
    a concurrency path, so it gets an automated test rather than a manual check.
  - Disabled mode never waits.
  - Idle identities are removed from the map.
- **`query_audit_tests.rs`:** the new fields serialize after `sql_hash`; `admission_cost` is
  absent when admission is disabled.

## Manual Verification

The end-to-end wiring (env → scheduler → `execute_query` → permit release on stream drop) runs
across the real gRPC handler. If it broke, queries would visibly hang or never queue the next
time anyone ran the steps below.

1. `python3 local_test_env/ai_scripts/start_services.py` with `MICROMEGAS_ADMISSION_BUDGET_MB=64`.
2. From Python, fire 20 concurrent `SELECT * FROM log_entries` with one identity's attribution
   headers, and 1 concurrent query with a second identity.
3. `micromegas-query "SELECT msg FROM log_entries WHERE target = 'flightsql_query_audit' ORDER BY time DESC LIMIT 25" --begin 5m`.
   Expected: the second identity's `queue_wait_ms` is near 0, the first identity's waits grow
   down its queue, and every query has status `ok`.
4. Cancel a client mid-wait (Ctrl-C). Expected: the remaining queued queries still complete
   (no leaked budget).

## Open Questions

1. **Identity granularity:** should the key also include `client` (web / python / grafana), so
   that one person's notebook and the same person's dashboard don't share a queue?
2. **Per-identity weights:** does a shared dashboard service account need a larger share (a
   weight in the least-in-flight comparison), or is equal share per identity the intended QoS?
3. **Default budget:** keep it disabled by default (estimate-only), or derive it from the
   DataFusion pool size × a factor once calibration data exists?
4. **S1 scan statistics (§0):** open a separate issue for `Inexact` `num_rows`/`total_byte_size`
   from partition metadata, so hash joins build on the smaller side?
