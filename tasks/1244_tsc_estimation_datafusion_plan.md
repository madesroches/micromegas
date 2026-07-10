# Move `ThreadSpansView::jit_update` off Postgres onto DataFusion Views Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1244

## Overview

`ThreadSpansView::jit_update` is the only remaining view path that reads process/stream/block
metadata directly from Postgres. Issue #1244 targets its worst offender — the raw `blocks`
full-table scan used to estimate TSC frequency — but the broader project direction is to rely on
Postgres as little as possible (its read performance and scalability are the bottleneck we are
moving away from). Every piece of metadata this function needs is already exposed by a DataFusion
view aggregated from the blocks parquet data, so this plan converts **all three** Postgres reads in
`jit_update` to DataFusion:

| Current Postgres read | Replacement |
|---|---|
| `make_time_converter_from_db` → `SELECT ... FROM blocks ...` (unprunable full scan) | `find_process_with_latest_timing` (reads `max(end_ticks)`/`max(end_time)` from the `processes` view) + `make_time_converter_from_latest_timing` |
| `find_process` → `SELECT ... FROM processes` (PK read) | same `find_process_with_latest_timing` call (also from the `processes` view) |
| `find_stream` → `SELECT ... FROM streams` (PK read) | new `find_stream_from_view` (reads the `streams` view) |

`net_spans_view` and `async_events_view` already use `find_process_with_latest_timing` /
`make_time_converter_from_latest_timing`; this converges `thread_spans` onto that pattern and adds
the symmetric stream lookup. The `blocks` scan is the highest-value removal (it does not prune on
`insert_time` and would fan out across every partition once `blocks` is hourly-partitioned), but
folding in the `streams`/`processes` PK reads keeps the whole path off Postgres, consistent with
the direction, at no extra structural cost (we already have to inject the `ViewFactory`).

The task is independent of the Aurora partitioning epic and can land at any time; it lowers risk
for the partitioning work.

## Current State

### `ThreadSpansView::jit_update` (`rust/analytics/src/lakehouse/thread_spans_view.rs:255`)

1. bails if `query_range` is `None` (query range is mandatory for this view — line 261);
2. `find_stream(db_pool, self.stream_id)` → `stream` (Postgres `streams` table);
3. `find_process(db_pool, &stream.process_id)` → `process` (Postgres `processes` table);
4. `make_time_converter_from_db(db_pool, &process)` → `convert_ticks` (Postgres `blocks` scan);
5. `generate_stream_jit_partitions(config, lakehouse, &blocks_view, &query_range, stream, process)`
   — **stream-scoped**; threads the full `Arc<StreamMetadata>` into every `PartitionSourceBlock`,
   where it is later consumed by `make_call_tree` to parse thread block payloads. So the complete
   `StreamMetadata` (with `dependencies_metadata`/`objects_metadata`/`properties`) is genuinely
   required, not just the id;
6. loops `update_partition(...)` per partition.

`ThreadSpansView` (line 59) holds only `view_set_name`, `view_instance_id`, `stream_id`; unlike
`NetSpansView`/`AsyncEventsView` it carries no `ViewFactory`, and `ThreadSpansViewMaker` (line 41)
is a unit struct `{}`.

### `make_time_converter_from_db` (`rust/analytics/src/time.rs:27`)

Fallback that runs the raw `SELECT end_time, end_ticks FROM blocks WHERE process_id = $1 ORDER BY
end_time DESC LIMIT 1` when `tsc_frequency <= 0`. `thread_spans` is its only non-test caller (and
no test calls it).

### The DataFusion helpers already in place

- `find_process_with_latest_timing` (`rust/analytics/src/metadata.rs:225`) builds a session
  context (`LivePartitionProvider`) and runs `SELECT ..., last_block_end_ticks, last_block_end_time
  FROM processes WHERE process_id = '...'`, returning `(ProcessMetadata, i64, DateTime<Utc>)`. It
  needs an `Arc<ViewFactory>`.
- `make_time_converter_from_latest_timing` (`rust/analytics/src/time.rs:89`) builds `ConvertTicks`
  from that timing.

### The `streams` view (`rust/analytics/src/lakehouse/streams_view.rs`)

A `SqlBatchView` grouping the blocks parquet data by `stream_id`, exposing exactly the columns
`StreamMetadata` needs — `stream_id`, `process_id`, `dependencies_metadata`, `objects_metadata`,
`tags`, `properties` (plus `insert_time`, `format`, `last_update_time`). Column Arrow types
(inherited from `blocks_view.rs:176-184`): `dependencies_metadata`/`objects_metadata` = `Binary`
(CBOR), `tags` = `List<Utf8>`, `properties` = the JSONB properties column.

### Existing Arrow → `StreamMetadata` extraction (duplication to reuse)

The exact reconstruction of a `StreamMetadata` from an Arrow row of `streams.*` columns already
exists in three places, all reading the **blocks/source** view (prefixed column names):
- `generate_process_jit_partitions_segment` — `jit_partitions.rs:300-339` (full: deps, objs,
  tags, properties via `properties_column_by_name(...).jsonb_value(...)`);
- `parse_block_table_function.rs:145` (partial: tags/properties left empty);
- `partition_source_data.rs:190`.

### Factory wiring (`rust/analytics/src/lakehouse/view_factory.rs:302-337`)

`thread_spans` is registered early (lines 306-309, `Arc::new(ThreadSpansViewMaker {})`), **before**
the completed `factory_arc` exists. `net_spans`/`async_events` are registered later (lines 322-332)
against `updated_factory.clone()`, which is why they carry a full `ViewFactory`.

### Non-circularity / data availability

By the time `thread_spans` is materialized, the blocks parquet view is already populated — the
same `jit_update` reads it a few lines later via `generate_stream_jit_partitions`. Both the
`processes` and `streams` views aggregate from that same blocks view, so reading them here has the
**same materialization semantics as the already-shipped `find_process_with_latest_timing`** used by
`net_spans`/`async_events`. No new ordering dependency beyond what those views already assume.

`streams_view` and `processes_view` are constructed identically (`view_factory.rs:274-301`), pushed
adjacently into the same `global_views` vec, and are both `SqlBatchView`s aggregating from the same
`blocks` source with the identical no-op `jit_update` (`sql_batch_view.rs:203-209`) and
`LivePartitionProvider` read path. Querying `FROM streams` therefore inherits the exact
on-demand/materialization guarantees already shipped for `FROM processes` via
`find_process_with_latest_timing` — confirmed by the code, not just by analogy.

## Design

Inject a `ViewFactory` into `ThreadSpansViewMaker`/`ThreadSpansView` (mirroring `NetSpansView`),
add a DataFusion-backed stream lookup symmetric to `find_process_with_latest_timing`, rewrite
`jit_update` to source all metadata from views, and delete the now-dead Postgres helpers.

### New: `find_stream_from_view` (`rust/analytics/src/metadata.rs`)

Symmetric to `find_process_with_latest_timing`. Builds a session context and runs:

```sql
SELECT stream_id, process_id, dependencies_metadata, objects_metadata, tags, properties
FROM streams
WHERE stream_id = '<uuid>'
```

then reconstructs a `StreamMetadata` from row 0. Signature:

```rust
pub async fn find_stream_from_view(
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    stream_id: &Uuid,
    query_range: Option<TimeRange>,
) -> Result<StreamMetadata>
```

Returns `StreamMetadata` (which already carries `process_id`), so `jit_update` no longer needs the
separate `find_process` call to discover the process id.

### DRY: shared `stream_metadata_from_batch_row` helper

Add a helper in `metadata.rs` that builds a `StreamMetadata` from one Arrow row of the `streams`
view's (unprefixed) columns:

```rust
pub fn stream_metadata_from_batch_row(
    batch: &RecordBatch,
    row: usize,
) -> Result<StreamMetadata>
```

Body mirrors `jit_partitions.rs:300-339` (Binary→CBOR for deps/objs, `List<Utf8>`→`Vec<String>`
for tags, `properties_column_by_name(...).jsonb_value(...)` for properties). Use it in
`find_stream_from_view`. The three existing blocks/source extraction sites read a mixed-prefix
schema (`stream_id`/`process_id` unprefixed, `dependencies_metadata`/`objects_metadata`/`tags`/
`properties`/`format` under `streams.`), so this helper — built for the uniformly-unprefixed
`streams` view — doesn't directly serve them; refactoring those sites is left out of this change
(see Trade-offs).

### Struct / constructor changes (`thread_spans_view.rs`)

```rust
pub struct ThreadSpansViewMaker { view_factory: Arc<ViewFactory> }
impl ThreadSpansViewMaker {
    pub fn new(view_factory: Arc<ViewFactory>) -> Self { Self { view_factory } }
}

pub struct ThreadSpansView {
    view_set_name: Arc<String>,
    view_instance_id: Arc<String>,
    stream_id: sqlx::types::Uuid,
    view_factory: Arc<ViewFactory>,
}
impl ThreadSpansView {
    pub fn new(view_instance_id: &str, view_factory: Arc<ViewFactory>) -> Result<Self> { ... }
}
```

`ViewMaker::make_view` forwards `self.view_factory.clone()`.

### Rewritten `jit_update` flow

```rust
// 1. mandatory query_range guard (unchanged)
let stream = Arc::new(
    find_stream_from_view(lakehouse.clone(), self.view_factory.clone(),
                          &self.stream_id, query_range).await?);
let (process, last_block_end_ticks, last_block_end_time) =
    find_process_with_latest_timing(lakehouse.clone(), self.view_factory.clone(),
                                    &stream.process_id, query_range).await?;
let process = Arc::new(process);
let convert_ticks =
    make_time_converter_from_latest_timing(&process, last_block_end_ticks, last_block_end_time)?;
// generate_stream_jit_partitions(...) + update_partition loop unchanged
```

`find_process`, `find_stream`, and `make_time_converter_from_db` are no longer referenced here.

### Factory registration move (`view_factory.rs`)

Remove the early `thread_spans` registration; add it alongside `net_spans`/`async_events`:

```rust
updated_factory.add_view_set(
    String::from("thread_spans"),
    Arc::new(ThreadSpansViewMaker::new(Arc::new(updated_factory.clone()))),
);
```

Nothing between the old and new registration points depends on the `thread_spans` view set
(`log_stats` is log-scoped), so the move is behavior-preserving, and it mirrors the existing
snapshot-clone pattern used for `net_spans`/`async_events`.

### Delete dead Postgres helpers

- `make_time_converter_from_db` (`time.rs`) — no remaining callers; delete it and drop the
  now-unused `sqlx::Row` import if nothing else in the file uses it.
- `find_stream` (`metadata.rs`) — `thread_spans` was its only caller; delete it once confirmed
  unused. `stream_metadata_from_row` (`metadata.rs:125`) has exactly one caller, `find_stream`
  itself (`metadata.rs:166`), so it becomes dead code once `find_stream` is deleted; delete it in
  the same step. `find_process` **stays** (still used by `log_view`, `metrics_view`, `images_view`,
  `otel/spans_view`).

## Implementation Steps

1. **`rust/analytics/src/metadata.rs`**
   - Add `stream_metadata_from_batch_row(batch, row)` helper (DRY extraction, `streams` view only).
   - Add `find_stream_from_view(lakehouse, view_factory, stream_id, query_range)` using it.
   - Delete `find_stream` once its only caller is gone (step 2), and delete
     `stream_metadata_from_row` alongside it (its only caller is `find_stream`).
2. **`rust/analytics/src/lakehouse/thread_spans_view.rs`**
   - Imports: drop `find_process`, `find_stream`, `make_time_converter_from_db`; add
     `find_stream_from_view`, `find_process_with_latest_timing`,
     `make_time_converter_from_latest_timing`, `view_factory::ViewFactory`.
   - Add `view_factory` to `ThreadSpansViewMaker` (+ `new`) and `ThreadSpansView` (+ updated `new`);
     forward it in `make_view`.
   - Rewrite the metadata/timing section of `jit_update` (above); keep
     `generate_stream_jit_partitions` and the `update_partition` loop intact.
3. **`rust/analytics/src/lakehouse/view_factory.rs`**
   - Move the `thread_spans` registration down to `ThreadSpansViewMaker::new(Arc::new(updated_factory.clone()))`.
4. **`rust/analytics/src/time.rs`**
   - Delete `make_time_converter_from_db`; drop now-unused imports.
5. (Out of scope) `jit_partitions.rs`, `parse_block_table_function.rs`, `partition_source_data.rs`
   still read the blocks/source schema (mixed-prefix columns) and are left as-is;
   `stream_metadata_from_batch_row` is not a drop-in replacement for them (see Trade-offs).
6. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, analytics tests.

## Files to Modify

- `rust/analytics/src/metadata.rs`
- `rust/analytics/src/lakehouse/thread_spans_view.rs`
- `rust/analytics/src/lakehouse/view_factory.rs`
- `rust/analytics/src/time.rs`

## Trade-offs

- **Scope: all three reads vs. only the `blocks` scan.** The issue only mandates removing the
  `blocks` scan. Converting the `streams`/`processes` PK reads too is a deliberate expansion driven
  by the "minimize Postgres" direction; it is nearly free because the `ViewFactory` injection and
  DataFusion session are needed for the timing change regardless. If a reviewer prefers a minimal
  diff, the stream conversion (new `find_stream_from_view`) can be dropped and `find_stream` kept —
  the `blocks`-scan removal stands alone.
- **`max(end_ticks)`/`max(end_time)` taken independently** vs. the old single latest-by-`end_time`
  row. For a monotonic clock these coincide, and the value only feeds a TSC-frequency *estimate*
  used when `tsc_frequency` is missing. This already matches `net_spans`/`async_events`.
- **Two DataFusion queries (streams + processes)** vs. one Postgres round-trip pair. Heavier per
  call, but this is a low-frequency JIT path and the win is removing the unprunable `blocks` scan
  and shedding Postgres load. Not worth folding into a single joined query (added coupling, less
  reuse of the existing `find_process_with_latest_timing`).
- **Shared extraction helper vs. inlining.** `stream_metadata_from_batch_row` avoids a 4th copy of
  the same Arrow decode for the `streams` view. It does not de-duplicate the existing three
  blocks/source sites (`jit_partitions.rs:300-309`, `parse_block_table_function.rs:120-141`,
  `partition_source_data.rs:179-199`): those read a mixed-prefix schema (`stream_id`/`process_id`
  unprefixed, the rest under `streams.`), which this single-schema helper doesn't cover. Unifying
  them would need a different helper shape (e.g. explicit column names per field, or two prefix
  arguments) and is left out of this change.
- **Deleting `make_time_converter_from_db`/`find_stream`** vs. leaving them: deleting removes
  dead, unpruned-Postgres-read helpers so nothing reintroduces them by habit. `find_process`,
  `make_time_converter_from_block_meta`, `make_time_converter_from_latest_timing`, and
  `stream_metadata_from_row` are left untouched (still used).

## Documentation

No user-facing behavior change and no doc page describes the TSC-estimation fallback or this
metadata path — no documentation updates required. If the Aurora partitioning epic tracks
remaining Postgres `blocks` reads, tick this one off there.

## Testing Strategy

- `cargo test -p micromegas-analytics` — existing suite, notably `parse_alloc_test.rs` (materializes
  `thread_spans` end-to-end) and `time_tests.rs` (`make_time_converter_from_latest_timing`).
- Confirm dead code is gone: `grep -rn "make_time_converter_from_db\|find_stream\b" rust/` returns
  nothing (except the new `find_stream_from_view`) after the change.
- `cargo clippy --workspace -- -D warnings` for dropped imports.
- Manual smoke: materialize `thread_spans` for a process with `tsc_frequency <= 0` and confirm span
  begin/end times match the previous Postgres-fallback output; also confirm call-tree parsing still
  works (validates that `StreamMetadata.dependencies_metadata`/`objects_metadata` reconstructed from
  the `streams` view match the Postgres-sourced values).

## Open Questions

1. **Scope confirmation** — proceed with converting all three reads (recommended, per direction),
   or land only the mandated `blocks`-scan removal and defer the stream conversion?
