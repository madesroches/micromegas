# FlightSQL Per-Query Audit Log Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1288

## Overview

The FlightSQL service already records request attribution (client type, user, email, requested
range, full SQL) as free-text `info!` lines and per-query cost as untagged `imetric!` metrics, but
the two signals never meet: the metrics carry an empty `PropertySet` (nothing to `GROUP BY`) and the
`execute_query` span is high-frequency with a static name. Answering "which clients/users are
responsible for the slowest and most expensive queries?" therefore requires fragile timestamp
correlation between a log line and a metric within the same process.

This plan emits **one structured JSON log line per query, at completion**, under a dedicated
`flightsql_query_audit` log target. A free-text `msg` has no cardinality constraint, so it carries
both the high-cardinality attribution (SQL, email) and the per-stage cost (durations, output rows,
bytes scanned) in one self-contained record, queryable with the existing JSONB UDFs. Two supporting
changes make `bytes_scanned` meaningful: retain the physical plan in `execute_query` so DataFusion's
own plan metrics can be read after the stream drains, and make the lakehouse parquet reader record
its byte reads into the `ExecutionPlanMetricsSet` it is currently handed and ignores.

## Current State

### `execute_query` and the completion-tracking stream

`rust/public/src/servers/flight_sql_service_impl.rs`:

- `execute_query` (`:173`–`:328`) parses the SQL and `query_range`, resolves attribution via
  `validate_and_resolve_user_attribution_grpc` (`:215`), reads `client_type` (`:217`), and logs a
  free-text `info!` with full attribution (`:225`–`:239`).
- It times four stages with `now()` (raw TSC ticks): `context_init_duration` (`:242`–`:252`),
  `planning_duration` (`:255`–`:260`), `execution_duration` (`:275`–`:290`, which measures only
  **stream construction**, not drain), and `total_setup_duration` (`:293`). These are emitted as
  untagged `imetric!(name, "ticks", value)` (`:296`–`:307`).
- It runs `df.execute_stream()` (`:277`), which **drops the `ExecutionPlan`** — so plan metrics can
  never be read.
- The response is wrapped in a `.map()` that emits `query_duration_with_error` on error (`:310`–
  `:322`) and then in `CompletionTrackedStream` (`:74`–`:124`), whose `poll_next` emits
  `query_duration_total` / `query_completed_successfully` on clean end (`:111`–`:118`) and
  `query_duration_with_error` / `query_failed` on the first error (`:100`–`:107`). This completion
  arm is where total duration and success/error are known — the natural emission point for the audit
  record.

`now()` (`rust/tracing/src/time.rs:39`) returns raw TSC ticks; `frequency()` (`:72`) is unreliable
on x86 (`tsc_frequency()` can return 0). So the existing tick metrics are converted to time only
downstream, using the per-process `tsc_frequency` stored in process metadata. For self-contained
millisecond fields in the audit JSON, the audit path must measure wall-clock with
`std::time::Instant` instead of deriving ms from ticks.

### Attribution shape

`micromegas_auth::user_attribution::UserAttribution` (`rust/auth/src/user_attribution.rs:14`) has
`user_id: String`, `user_email: String`, `user_name: Option<String>`, `service_account:
Option<String>`. `service_account.is_some()` is the service-account flag.

### Lakehouse parquet reader ignores its metrics set

`rust/analytics/src/lakehouse/reader_factory.rs`:

- `ReaderFactory::create_reader` (`:53`–`:74`) receives `_metrics: &ExecutionPlanMetricsSet` and
  drops it (`// todo: don't ignore metrics` `:61`).
- `ParquetReader` (`:79`–`:153`) reads bytes in `get_bytes` (`:88`) and `get_byte_ranges` (`:114`)
  but only `debug!`-logs the byte counts; nothing is recorded into a metrics set. Consequently
  `bytes_scanned` reads as **zero** for lakehouse scans.

### DataFusion 54 APIs (verified against the vendored source)

- `DataFrame::create_physical_plan(&self) -> Result<Arc<dyn ExecutionPlan>>`
  (`datafusion-54.0.0/src/dataframe/mod.rs:301`) and `DataFrame::task_ctx(&self) -> TaskContext`
  (`:1557`). Both borrow `&self`, so the plan can be built and kept without consuming `df`.
- Free function `datafusion::physical_plan::execute_stream(plan, task_ctx) ->
  Result<SendableRecordBatchStream>` (re-exported into `datafusion::prelude` and `dataframe`).
  `DataFrame::execute_stream` is literally `execute_stream(self.create_physical_plan().await?,
  Arc::new(self.task_ctx()))` — we inline that but keep `plan`.
- `ExecutionPlan::metrics(&self) -> Option<MetricsSet>` and `ExecutionPlan::children()`.
- `MetricsSet::output_rows() -> Option<usize>`, `MetricsSet::sum_by_name(name) ->
  Option<MetricValue>`; `MetricValue::as_usize() -> usize` (metrics live in
  `datafusion_physical_expr_common::metrics`, re-exported at `datafusion::physical_plan::metrics`).
- `datafusion::datasource::physical_plan::ParquetFileMetrics::new(partition, filename, metrics) ->
  ParquetFileMetrics` (re-exported at `datasource/physical_plan/mod.rs:36`); its `bytes_scanned:
  Count` field is what the default reader `.add(n)`s per read (`datafusion-datasource-parquet-54.0.0/
  src/reader.rs:104`, `:122`). `Count::add(usize)` / `Count::value() -> usize`.

## Design

### 1. New module `servers/query_audit.rs`

Add `rust/public/src/servers/query_audit.rs` (registered in `rust/public/src/servers/mod.rs`)
holding the record type, its JSON serialization, and the plan-metric aggregation helper — keeping
the service file lean and the logic unit-testable from `rust/public/tests/`.

```rust
use datafusion::physical_plan::ExecutionPlan;

/// Aggregated DataFusion plan metrics for one query, read after the stream drains.
pub struct ScanMetrics {
    pub output_rows: Option<u64>,
    pub bytes_scanned: u64,
}

/// Walk the physical-plan tree: output_rows from the root node (final result grain),
/// bytes_scanned summed across every node (leaf DataSourceExec nodes carry it).
pub fn aggregate_scan_metrics(plan: &dyn ExecutionPlan) -> ScanMetrics {
    fn sum_bytes(plan: &dyn ExecutionPlan) -> u64 {
        let mut total = plan
            .metrics()
            .and_then(|m| m.sum_by_name("bytes_scanned"))
            .map(|v| v.as_usize() as u64)
            .unwrap_or(0);
        for child in plan.children() {
            total += sum_bytes(child.as_ref());
        }
        total
    }
    ScanMetrics {
        output_rows: plan.metrics().and_then(|m| m.output_rows()).map(|r| r as u64),
        bytes_scanned: sum_bytes(plan),
    }
}
```

The audit record is a `serde::Serialize` struct so field names/absence are explicit and the JSON is
stable. `serde`/`serde_json` are already deps of `public` (`Cargo.toml:51`,`:73`).

```rust
#[derive(serde::Serialize)]
pub struct QueryAuditRecord<'a> {
    pub client: &'a str,
    pub user: &'a str,
    pub email: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'a str>,
    pub service_account: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_account_name: Option<&'a str>,
    pub sql: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range_begin: Option<String>, // RFC3339
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    pub context_init_ms: f64,
    pub planning_ms: f64,
    pub execution_ms: f64, // stream construction (matches query_execution_duration semantics)
    pub setup_ms: f64,     // parse+attribution+context+planning+stream-build (query_setup_duration)
    pub total_ms: f64,     // end-to-end incl. drain
    pub status: &'static str, // "ok" | "error"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_rows: Option<u64>,
    pub bytes_scanned: u64,
}
```

### 2. Thread attribution + durations + plan into the stream

`execute_query` computes attribution and stage durations before the stream exists but total duration,
status, and plan metrics only settle at completion. Carry the former into the stream wrapper so the
completion arm can assemble a complete record.

- Capture `let request_start = std::time::Instant::now();` at function entry (alongside the existing
  `begin_request = now()`, which the tick metrics keep using — those emissions are left unchanged to
  avoid regressing existing dashboards).
- Measure each audit stage duration with `Instant` deltas → `f64` ms (`d.as_secs_f64() * 1000.0`),
  in parallel with the existing `now()`-based tick timing. (Two cheap timer reads per stage; keeps
  the tick metrics byte-for-byte and gives the JSON reliable ms without depending on `frequency()`.)
- Build an owned `QueryAuditState` carrying the attribution strings, `sql`, range (as RFC3339
  strings), `limit`, the four stage ms values, `request_start`, and `plan: Arc<dyn ExecutionPlan>`.

### 3. Retain the physical plan

Replace `df.execute_stream()` (`:277`) with:

```rust
let task_ctx = std::sync::Arc::new(df.task_ctx());
let plan = df
    .create_physical_plan()
    .await
    .map_err(|e| status!("error creating physical plan", e))?;
let stream = datafusion::physical_plan::execute_stream(plan.clone(), task_ctx)
    .map_err(|e| Status::internal(format!("Error executing plan: {e:?}")))?
    .map_err(|e| FlightError::ExternalError(Box::new(e)));
```

`plan` (an `Arc`) is cloned into `QueryAuditState`; DataFusion plan metrics settle only after the
stream is fully drained, which is exactly when the completion arm fires. The `df.limit(...)` handling
(`:262`–`:272`) stays before `create_physical_plan` so the limit is planned in. `schema` is still
taken from `df.schema()` before the plan is built.

### 4. Emit the audit record in the completion arm

Move the audit emission into `CompletionTrackedStream`, which already owns the success/error
completion transitions. Give it an `Option<QueryAuditState>` (taken on first completion so it emits
exactly once, matching the existing `completed` guard):

- **Success** (`Poll::Ready(None)`, `:111`): `status = "ok"`, `error = None`.
- **Error** (`Poll::Ready(Some(Err))`, `:100`): `status = "error"`, `error = Some(err.to_string())`.

In both arms, after the existing `imetric!`s:

```rust
if let Some(state) = self.audit.take() {
    let scan = aggregate_scan_metrics(state.plan.as_ref());
    let total_ms = state.request_start.elapsed().as_secs_f64() * 1000.0;
    let record = QueryAuditRecord { /* from state + scan + status/error + total_ms */ };
    match serde_json::to_string(&record) {
        Ok(json) => info!(target: "flightsql_query_audit", "{}", json),
        Err(e) => warn!("failed to serialize query audit record: {e}"),
    }
}
```

Emit the JSON as a **format argument** (`"{}", json`), never as the format string, so literal `{`/`}`
in the SQL/JSON are not misparsed by `format_args!`. The `target:` literal routes the line to the
dedicated `flightsql_query_audit` target (`log!` macro `rust/tracing/src/macros.rs:252`), keeping it
filterable independently of the chattier per-request logs.

The redundant intermediate `.map()` error wrapper (`:310`–`:322`) can be dropped: it only emitted
`query_duration_with_error`, which `CompletionTrackedStream`'s error arm already emits. The stream
built at `:277` (mapped to `FlightError`) feeds directly into `CompletionTrackedStream`.

The existing free-text attribution `info!` (`:225`–`:239`) is **retained** — it fires at query start
(useful for in-flight visibility, while the audit record only appears at completion). The audit
record is the structured, completion-time superset for cost attribution.

### 5. Make the lakehouse reader report bytes scanned

`rust/analytics/src/lakehouse/reader_factory.rs`: mirror DataFusion's default reader by recording
byte reads into the handed metrics set.

- In `create_reader`, build the standard metric and pass its `Count` to the reader:

```rust
use datafusion::datasource::physical_plan::ParquetFileMetrics;
// ...
let file_metrics =
    ParquetFileMetrics::new(partition_index, partitioned_file.path().as_ref(), metrics);
Ok(Box::new(ParquetReader {
    // ...existing fields...
    bytes_scanned: file_metrics.bytes_scanned,
}))
```

  `partition_index` and `metrics` are already parameters (currently `_`-prefixed). Using
  `ParquetFileMetrics` keeps the metric name/labels identical to DataFusion's own reader, so
  `sum_by_name("bytes_scanned")` and EXPLAIN ANALYZE both see it.

- Add `pub bytes_scanned: datafusion::physical_plan::metrics::Count` to `ParquetReader` and, in
  `get_bytes` / `get_byte_ranges`, call `self.bytes_scanned.add(bytes_requested as usize)` /
  `.add(total_bytes as usize)` right where the counts are already computed (`:94`, `:121`). The
  existing `debug!` byte logging stays.

Note: this factory is typically wrapped by the L1 object cache, so `bytes_scanned` counts bytes the
parquet reader requested from its (possibly cache-backed) store — i.e. logical bytes the query
needed, not necessarily bytes fetched from origin. That is the right grain for per-query cost
attribution; the process-global `range_cache_origin_block_bytes` metric remains the origin-fetch
signal.

## Implementation Steps

1. **Reader byte accounting** — `rust/analytics/src/lakehouse/reader_factory.rs`: add the
   `bytes_scanned: Count` field to `ParquetReader`, build it via `ParquetFileMetrics::new` in
   `create_reader` (un-prefix `partition_index`/`metrics`), and `.add(...)` in both read methods.
   Remove the `// todo: don't ignore metrics` comment.
2. **Audit module** — add `rust/public/src/servers/query_audit.rs` with `ScanMetrics`,
   `aggregate_scan_metrics`, and `QueryAuditRecord`; register `mod query_audit;` in
   `rust/public/src/servers/mod.rs`.
3. **Retain plan + thread state** — in `execute_query`: capture `request_start: Instant`, add
   `Instant`-based ms timing per stage, switch to `create_physical_plan` + free-function
   `execute_stream` keeping `plan`, and build `QueryAuditState`.
4. **Emit at completion** — extend `CompletionTrackedStream` with `audit: Option<QueryAuditState>`;
   assemble and `info!(target: "flightsql_query_audit", "{}", json)` in both completion arms; drop
   the redundant intermediate `.map()` error wrapper.
5. **Tests** — see Testing Strategy.
6. **Docs** — see Documentation.

## Files to Modify

- `rust/analytics/src/lakehouse/reader_factory.rs` — record `bytes_scanned` into the metrics set.
- `rust/public/src/servers/query_audit.rs` — **new**: record struct, JSON serialization, metric
  aggregation.
- `rust/public/src/servers/mod.rs` — register the new module.
- `rust/public/src/servers/flight_sql_service_impl.rs` — retain plan, thread state, emit audit line.
- `rust/public/tests/query_audit_tests.rs` — **new**: unit tests (see below).
- `mkdocs/docs/query-guide/` (+ `mkdocs.yml` nav if a new page) — document the audit target.

## Trade-offs

- **Structured log line vs. tagged metrics.** Tagging the cost metrics can't carry SQL text /
  fingerprints (property values are interned `&'static str`; unbounded cardinality would blow up the
  intern table). A free-text JSON `msg` has no such limit and keeps one row per query at the natural
  grain. Chosen per the issue.
- **`Instant` ms alongside tick metrics vs. converting ticks.** Deriving ms from ticks needs
  `frequency()`, which is unreliable on x86. Measuring the audit stage durations with `Instant`
  gives robust ms and leaves the existing tick metrics (and their downstream conversion) untouched,
  at the cost of a second cheap timer read per stage.
- **Emit at completion vs. at start.** Bytes/rows/total only exist at completion. The start-time
  `info!` is kept for in-flight visibility; the audit record is the completion-time superset.
- **Reader records requested bytes (cache-wrapped) vs. origin bytes.** Requested bytes is the
  correct per-query grain; origin-fetch bytes stay a separate process-global signal.
- **`ParquetFileMetrics::new` vs. a bare hand-built `Count`.** The former also creates pruning /
  metadata-timing metrics (mostly zero here) but keeps names/labels identical to DataFusion's own
  reader, so aggregation and EXPLAIN ANALYZE agree. Worth the few extra zero-valued metric entries.

## Documentation

- Add a "Query audit log" section under `mkdocs/docs/query-guide/` (extend an existing page or add a
  new one wired into `mkdocs/mkdocs.yml`) describing the `flightsql_query_audit` target, the JSON
  field set, and the always-use-a-bounded-time-range-plus-`target`-filter guidance. Include the
  attribution/cost `GROUP BY` example from the issue (uses `jsonb_parse`, `jsonb_get`,
  `jsonb_as_string`, `jsonb_as_f64`, `jsonb_as_i64` — all confirmed registered).

## Testing Strategy

- **Unit (`rust/public/tests/query_audit_tests.rs`)**:
  - `QueryAuditRecord` serialization: required fields present; `skip_serializing_if` omits
    `None`/absent optionals; `service_account` bool set correctly; a SQL string containing `{`/`}`
    and quotes round-trips through `serde_json` and re-parses.
  - `aggregate_scan_metrics` on a small hand-built plan (or a real `create_physical_plan` over a
    tiny in-memory table): `output_rows` from the root, `bytes_scanned` summed across nodes; empty
    metrics → `output_rows: None`, `bytes_scanned: 0`.
- **Integration**: run a query through the FlightSQL service against local test env
  (`python3 local_test_env/ai_scripts/start_services.py`), then verify one
  `target = 'flightsql_query_audit'` row per query via `micromegas-query`, that `jsonb_parse(msg)`
  yields the expected keys, and that `bytes_scanned > 0` for a query that scans lakehouse partitions.
- **Regression**: `cargo test`, `cargo clippy --workspace -- -D warnings`, `cargo fmt`; confirm the
  existing cost metrics (`query_duration_total`, `query_setup_duration`, ...) are still emitted
  unchanged.

## Open Questions

- **Page vs. section for docs** — new dedicated page under `query-guide/` or a section appended to an
  existing page (e.g. the schema-reference/functions page). Default: extend an existing query-guide
  page unless it grows too large. Resolvable at implementation time.
- **Fingerprint field** — the issue suggests an optional normalized fingerprint (literals stripped)
  alongside `sql`. Deferred: raw `sql` satisfies drill-down; a fingerprint can be added later as an
  additive field without breaking consumers.
