# jit: process insert-time bounds require a full blocks_view data scan

## Problem

`generate_process_jit_partitions` (`rust/analytics/src/lakehouse/jit_partitions.rs:391-459`)
determines a process's `insert_time` bounds — used to slice the segment loop into
`max_insert_time_slice` chunks — with a DataFusion query instrumented as
`collect_insert_time_range` (line 439):

```sql
SELECT MIN(insert_time) as min_insert_time, MAX(insert_time) as max_insert_time
FROM source
WHERE process_id = '{process_id}'
AND array_has("streams.tags", '{stream_tag}')
AND begin_time <= '{end_range_iso}'
AND end_time >= '{begin_range_iso}';
```

This query runs over every `blocks_view` partition overlapping `query_time_range`
(fetched via `LivePartitionProvider::fetch`, lines 400-412), which prunes partitions
only on their `min_event_time`/`max_event_time` metadata in `lakehouse_partitions` —
not on `process_id`. `blocks_view` partitions are daily and hold blocks for *every*
process/stream active that day, ordered by `insert_time` rather than clustered by
process (`blocks_view.rs:38-46`), so there's no partition- or row-group-level pruning
available for `process_id`. DataFusion must open and decode each matching partition's
Parquet data in full and filter row-by-row down to the one process. This is reported
as slow in practice, especially for wide `query_time_range`s (multi-day/week) on
deployments with many processes/streams active per day.

## Root cause

A per-process lookup is answered by scanning materialized lakehouse Parquet
partitions that aggregate *all* processes, with no way to prune by `process_id`
at the partition or row-group level.

Note: the query does **not** stay within the lakehouse's own query path by design —
any fix must too. `blocks`/`streams`/`processes` are the raw ingestion tables, not a
durable historical source: `delete_old_data` (`rust/analytics/src/delete.rs:152-170`)
prunes rows from them directly (`delete_expired_blocks`, `delete_empty_streams`,
`delete_empty_processes`), on the same maintenance pass that retires expired lakehouse
partitions. Querying those raw tables directly (bypassing DataFusion/`query_partitions`)
would tie correctness to that retention window and to Postgres being the answer path at
all, which cuts against how this system is meant to evolve — object storage/Parquet is
the durable source, Postgres is transient bookkeeping. Any fix should stay inside the
lakehouse/DataFusion query path.

## Solution directions

- **Extend partition-level metadata with per-process bounds.** `lakehouse_partitions`
  (queried via `LivePartitionProvider`/`PartitionCache`) already stores per-partition
  `min_event_time`/`max_event_time`/`begin_insert_time`/`end_insert_time`, computed once
  at materialization time and valid for as long as the partition exists. Adding a
  per-process (or per-stream) insert-time min/max at that same materialization step —
  even a coarse structure like a sorted list or bitmap of `process_id`s known to
  appear in the partition — would let `generate_process_jit_partitions` skip
  partitions that can't contain this process without reading their Parquet data.
  Larger change: touches partition-write code (`write_partition.rs`) and the
  `lakehouse_partitions` schema, but keeps everything inside the lakehouse's own
  metadata catalog rather than the raw ingestion tables.
- **Drop the tightened `insert_time_range` computation and walk `query_time_range`
  directly.** The only reason for `collect_insert_time_range` is to avoid iterating
  over segments where this process has no data. Once the per-segment loop no longer
  does its own Postgres round-trip (see `process_jit_partitions_optimization_plan.md`),
  an empty segment costs a cheap in-memory `filter_insert_range` plus a `query_partitions`
  call over zero partitions — no network round trip, no Parquet I/O. Walking the full
  `query_time_range` in `max_insert_time_slice` chunks (instead of computing a tighter
  bound first) trades the upfront full-data scan for some number of cheap no-op
  segments, without introducing any new metadata. Worth confirming empty-partition-list
  `query_partitions` calls are in fact near-zero-cost before committing to this over the
  metadata-extension option above.

Out of scope for `process_jit_partitions_optimization_plan.md`: that plan only
addresses the per-segment loop *after* `insert_time_range` is already known; this
issue is entirely upstream of it (computing `insert_time_range` itself).

## Files to change

- `rust/analytics/src/lakehouse/jit_partitions.rs` — `generate_process_jit_partitions`,
  lines 399-459 (the initial partition fetch and the `collect_insert_time_range` query).
- If pursuing the partition-metadata option: `rust/analytics/src/lakehouse/write_partition.rs`
  and the `lakehouse_partitions` table schema/migration.
