# Should `measures` / `log_entries` Declare a Scan Ordering or Be Sorted? — Research

Research spun out of [#1491](https://github.com/madesroches/micromegas/issues/1491), which asked
whether `measures`/`MetricsView` is a good candidate for a `ScanOrdering` declaration and how much
memory that would save. **Answers: no, and none.** The memory win the issue observed comes from the
scan's `repartition_file_scans` setting, not from any ordering — that fix is the separate plan
`tasks/1491_merge_scan_memory_plan.md`. This document records the ordering question so it does not
get re-derived.

## Q1: Can `measures` declare `ScanOrdering::Concatenated`? No — it is unsafe.

`View::get_scan_output_ordering`'s contract (`view.rs:150-209`) requires (a) rows within each
partition file already sorted by `columns`, and (b) partition ranges on the leading column not
overlapping. Both fail for `measures`:

- **(a) fails.** `measures` partitions are written by `BlockPartitionSpec::write`, which streams
  per-block row sets through `.buffer_unordered(nb_tasks)` (`block_partition_spec.rs:144`) — its own
  rustdoc says it "processes blocks individually and out of order". Rows land in block-completion
  order; there is no sort in the write path and no write-time check like
  `thread_spans_view::ensure_begin_non_decreasing`.
- **(b) fails for `time`.** Partitions are cut on *insert* time while `time` is event time, so a
  block's rows precede its insert time by the block's fill duration. Consecutive one-minute
  partitions overlap on `[min_event_time, max_event_time]`.
- **`insert_time` + `OrderingBounds::InsertTime`** satisfies (b) by construction but still fails (a),
  for the same `buffer_unordered` reason. This is what `blocks_view` gets right and `measures` cannot:
  `BlocksView`'s extract query ends in `ORDER BY blocks.insert_time, blocks.block_id`
  (`blocks_view.rs:68`), and its `merge_partitions` additionally requires every input to *carry* the
  recorded `sort_order` (`blocks_view.rs:198`).
- **`PerFile` would be a no-op.** It is gated by `Partition::certifies_sort_order`
  (`partitioned_execution_plan.rs:291`), and `BlockPartitionSpec` passes `None` for `sort_order`
  (`block_partition_spec.rs:92`), so the declaration degrades silently to `Unordered`.

**The failure mode is mixed, which is what makes it worse than a plain rejection.**
`sort_and_check_non_overlapping` (`partitioned_execution_plan.rs:83`) walks `partitions.windows(2)`
and hard-errors on the first overlapping *adjacent pair* — but a partition set with no such pair (a
single selected partition, or a query range picking a non-overlapping subset) passes with `Ok`, the
declared ordering is attached, and (a) still fails, so the query **silently returns mis-ordered
rows**. Most sampled `measures` traffic lands in that silent branch: JIT `view_instance` partitions
are typically a single small file per process.

Scope note: `get_scan_output_ordering` has exactly one consumer, `MaterializedView::scan`
(`materialized_view.rs:94`) — the **user-query** path. `QueryMerger`'s merge-side ordering is a
separate field set via `with_merge_scan_ordering` (`merge.rs:87-90`, defaulting to `Unordered`),
which `MetricsView` never calls. So declaring `Concatenated` would break `measures` *queries*, not
`measures` merges.

**All of the above holds for `log_entries` verbatim.** `LogView` overrides neither
`merge_partitions` nor `get_scan_output_ordering`, constructs the same `BlockPartitionSpec`
(`log_view.rs:127`) with the same `sort_order: None`, cuts on insert time, and groups its JIT
partitions under the same `BlockOrder::InsertTime` (`log_view.rs:192`). `async_events` and `images`
are the same shape again; `measures` and `log_entries` are simply the two large enough to matter.

## Q2: Should we add a real write-time sort? No — on evidence, not on effort.

A recorded sort would add what concatenation-order clustering cannot: exact row-group pruning,
`ORDER BY` elision, and a `PerFile` k-way streaming merge. The machinery already exists (#1392 built
`PerFile`, `certifies_sort_order`, and the `SortPreservingMergeExec` path, with `log_stats` as its
in-repo adopter). So the question is what it would buy.

### Query evidence — `measures`

6-hour sample of `flight-sql-srv`'s query audit log (`log_entries` where
`target = 'flightsql_query_audit'`, written by `rust/public/src/servers/query_audit.rs`) from a
production deployment, 548 queries touching `measures`:

- **Every one filters `WHERE name = '<literal>'`.** Nothing else is ever filtered — not `target`,
  `unit`, `stream_id`, `computer`, `username`, or `exe`. About half also `ORDER BY time`; the rest
  are `min`/`max`/`avg` aggregates.
- **546 of 548 go to per-process JIT view instances** (`view_instance('measures', <process_id>)`).
  **The global view — the one the hourly merge produces — received 2 queries in 6 hours.**
- Those queries are not scan-bound: a per-process partition is a few-MB file of which ~1 MB is read,
  and p50 latency (~2 s) is dominated by JIT freshness checking over a requested range far wider
  than the data the process holds.
- Caveat: 544 of the 546 come from one auto-refreshing dashboard against a single process. Real, but
  narrow.

### Query evidence — `log_entries` (opposite traffic, same answer)

Same source and window, 3178 audited queries referencing `log_entries`:

- **3171 of 3178 go to the global view**; 7 use `view_instance`. The exact inverse of `measures`, so
  here the merged population *is* the queried population.
- **Only 28 distinct SQL statements** account for all 3178 — dashboard traffic, refreshing.
- **What they filter:** 3169 on `level`; 3153 call `property_get(process_properties, '<key>')`;
  3026 `GROUP BY`; 289 `ORDER BY`; 11 mention `process_id`. **No high-cardinality scalar column is
  ever filtered.**
- **3170 of 3178 are on fresh data** (`query_time - range_end ≤ 1 hour`).
- **Window width vs. bytes scanned**, bucketing on the audit record's `range_end - range_begin`:

  | Requested window | Queries | Avg bytes scanned | Max | Total |
  |---|---|---|---|---|
  | ≤ 2 min | 2880 | 1.8 MB | 5 MB | 5.0 GB |
  | ≤ 1 hour | 272 | 175.8 MB | 315 MB | 46.7 GB |
  | ≤ 1 day | 19 | 856.9 MB | 1.6 GB | 15.9 GB |
  | > 1 day | 1 | 0 | 0 | 0 |

### Why the sort loses, for each view

- **Write-side cost is the same for both, and lands on the every-minute path.** Sorting at merge
  time is a blocking `SortExec` over the full ~2 GB — the blowup #1392 exists to remove. So it must
  go in `BlockPartitionSpec::write`, which today never materializes a partition at all
  (`block_partition_spec.rs:146-161`). Sorting means buffering a whole one-minute partition (~33 MB
  compressed, ~100-200 MB as Arrow) on top of the ~100 MB of concurrent block payloads that path
  already holds (`block_partition_spec.rs:107`) — trading an hourly spike for a per-minute one.
  `BlockPartitionSpec` is shared with `log_entries`, `async_events`, and `images`, so it would have
  to be opt-in per view, and the JIT population is written by a different path again
  (`jit_partitions::write_partition_from_blocks`) — two places to sort.
- **A sort scatters the per-block clustering the schema compresses on.** Within a block,
  `process_id`, `stream_id`, `block_id`, `exe`, `username`, `computer`, and `process_properties` are
  constant — 7 of the 14 columns in `metrics_table_schema()`, long dictionary runs RLE compresses to
  nearly nothing. Any sort key turns those into effectively random dictionary indices.
- **It does not help merge memory — it may hurt.** `PerFile` gives each input its own plan
  partition, so an hourly merge opens **k = 60** concurrent Parquet readers. That is more than
  today's `target_partitions`, not fewer. #1392 accepts that trade because it removes a blocking
  aggregate sort; `measures`' merge query is a bare `SELECT * FROM source`, so there is nothing to
  unblock.
- **`measures`: right key, wrong population.** If a sort ever happens the key is **`(name, time)`,
  not `time`** — `name` is the only column ever filtered and it is high-cardinality, so it is the
  only key that can prune. That overrides the instinct (and #1392 §7's `log_stats` reasoning) to
  lead with time, and a non-temporal leading key rules out `Concatenated` and forces `PerFile`. But
  the two populations are disjoint and the sort lands on the wrong one: global partitions have the
  row groups for a `name` key to bite and see ~2 queries per 6 hours, while JIT per-process
  partitions absorb all the traffic but are single small files bound by freshness checking.
- **`log_entries`: no candidate key at all.** The only scalar column ever filtered is `level`, which
  has six values (`micromegas_tracing::Level`) and cannot prune; everything else goes through
  `property_get` over `process_properties`, which is not a sortable scalar column. A `(time, ...)`
  key would prune nothing these dashboards ask for — they already narrow by time through the query
  range.
- **Row-group granularity floors any win.** Pruning cannot select less than one row group (128 Ki
  rows, `write_partition.rs:923`). Measured on a comparable high-volume metrics view in the same
  deployment: a `name` matching 0.0008% of rows cut scanned bytes only 4.7× (101 MB → 21.6 MB) on
  fresh one-minute partitions, because such a partition is only ~7 row groups.
- **Rollout would be self-healing but slow.** Existing partitions record `sort_order` NULL and never
  certify, so merges keep the unordered path until the whole retention window is re-materialized.

## Verdict

**No `ScanOrdering` declaration and no write-time sort, for either view.** Q1 is settled on
correctness. Q2 is settled on evidence: for `measures` the population that would benefit is barely
queried and the population that is queried is bound by something else; for `log_entries` there is no
key to sort by, and no sampled query asks for a window materially narrower than the partition it
reads — the ≤ 2-minute queries average 1.8 MB because they land on fresh one-minute partitions where
the partition already *is* the window, and the queries that do reach merged partitions want the
whole hour or the whole day.

The same evidence also bounds what the merge fix can claim: the sequential-scan change restores
time-local row groups in merged partitions (real, and free), but in this deployment neither view
collects on that pruning — for opposite reasons. That is why the merge plan measures the mechanism
with a synthetic narrow-window query rather than promising a query-latency win.

**Revisit if:** global-view query volume grows, a second deployment shows a different query mix, or
`measures` dashboards start filtering something other than `name`. If revisited, the key is
`(name, time)` and the mechanism is #1392's `PerFile`.

## Follow-ups this research points at (neither is about ordering)

1. **`measures` JIT freshness checking.** Per-process `view_instance` queries spend their time
   checking freshness over requested ranges far wider than the process's data, not scanning. No sort
   key or scan change addresses it.
2. **Global `log_entries` wide-window dashboard scans.** Of the 67.6 GB scanned by global
   `log_entries` queries in 6 hours, 62.6 GB comes from the 292 refreshes asking for a window wider
   than 2 minutes, over a 288 GB view.

Both are larger wins in the sampled workload than anything in the merge plan.

## Verifying the overlap claim against real data

The half of Q1 that real data can settle — run for `log_entries` as well:

```sql
SELECT count(*) AS overlapping_adjacent_pairs
FROM (
  SELECT max_event_time,
         lead(min_event_time) OVER (ORDER BY min_event_time, file_path) AS next_min
  FROM list_partitions()
  WHERE view_set_name = 'measures' AND view_instance_id = 'global'
) t
WHERE next_min < max_event_time;
```

Ordered by `min_event_time` (tiebreak `file_path`) to match the adjacency
`sort_and_check_non_overlapping` actually checks (`partitioned_execution_plan.rs:83-90` sorts by the
leading-column `begin` bound, not by `begin_insert_time`). A non-zero count is the number of
partition pairs the check would have errored on.
