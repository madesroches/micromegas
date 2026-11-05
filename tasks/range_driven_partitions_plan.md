# Range-Driven Partition Sizing and On-Demand Compaction Plan

Issue: [#1496](https://github.com/madesroches/micromegas/issues/1496)

## Overview

JIT views currently cut partitions into a fixed one-hour insert-time bucket
(`JitPartitionConfig::max_insert_time_slice`), whatever the query range. This plan sizes buckets
from the query range instead, using one aligned ladder (minute, 5 min, hour, 6 h, day). It also
promotes fine partitions into coarse ones on the read path, as an optional step that is skipped on
contention. Two things improve. A live "last minute" poll stops re-decoding the whole current hour
on every refresh, because only the open minute is rewritten. And a long-range query reads roughly
24 to 288 partitions (worst case just under 24 h) whatever its span, because closed history is promoted to the rung that range
calls for. The first cut targets the five `BlockOrder::InsertTime` JIT views (`log_entries`,
`measures`, `async_events`, `images`, otel `spans`).

## Current State

- **Fixed bucket.** `generate_process_jit_partitions` (`rust/analytics/src/lakehouse/jit_partitions.rs:901`)
  pre-queries `MIN/MAX(insert_time)` of blocks overlapping the event-time query range, then widens
  that range to `max_insert_time_slice` boundaries with `duration_trunc` (`:962-968`). It counts
  blocks per bucket (`fetch_bucket_counts`, `:457`, `COUNT(*)` only), packs buckets into batch
  queries (`batch_windows`, `:381`, a fixed `slice` step), splits the rows back per bucket
  (`split_into_buckets`, `:429`), and groups each bucket by `max_nb_objects` (20M) in
  `group_blocks_into_partitions` (`:243`). `generate_stream_jit_partitions` (`:763`) does the same
  for one stream. A spec's insert range is the inclusive min/max of its blocks' `insert_time`, not
  the bucket edges.
- **Live tail cost.** A "last minute" query of a live process truncates to the current hour, so
  every refresh that sees a new block re-decodes every block of that hour and rewrites the hour's
  partition.
- **Freshness (InsertTime views).** `find_up_to_date_partitions` → `resolve_up_to_date_fixpoint` →
  `spec_is_up_to_date` (`:1116-1191`). A spec is up to date when exactly one candidate row
  *overlaps* it and that row's object count is `>=` the spec's. This only works because every
  writer uses the same cut points. The `>=` is there so a stale spec that a wider partition already
  covers stays a no-op.
- **Write and retire.** `write_partition_from_blocks` (`:1399`) → `BlockPartitionSpec::write` →
  `insert_partition` (`write_partition.rs:439`). The insert takes a blocking advisory lock on the
  exact `(view, instance, begin, end)` key (`:474-505`), retires rows with `RetireMatch::Containment`
  (or `Overlap` for the two EventTime views, `write_partition.rs:137-176`), and inserts. The
  `lakehouse_partitions_no_overlap` exclusion constraint (`migration.rs:531`, half-open
  `tstzrange`, per `file_schema_hash`) catches overlapping writers of different ranges as SQLSTATE
  23P01. The error is translated at `write_partition.rs:619`.
- **Merge.** `create_merged_partition` (`merge.rs:400`) merges a `PartitionCache` subset through
  `View::merge_partitions` (default `SELECT * FROM source`, `view.rs:101`). It writes the sum of
  the inputs' `source_data_hash` and retires the inputs with `Containment`. The daemon uses it for
  global views on its fixed minute/hour/day schedule (`public/src/servers/maintenance.rs`).
- **Trigger.** `MaterializedView::scan` runs `jit_update` before `part_provider.fetch`
  (`materialized_view.rs:108-142`). A `jit_update` error fails the query. Read-only connections
  get a dedicated error (`:113-128`).
- **Duplication.** The five InsertTime views repeat the same `jit_update` body: find the process,
  call `generate_process_jit_partitions`, call `find_up_to_date_partitions`, then write the stale
  specs (e.g. `log_view.rs:149-211`).
- **Closed buckets are immutable.** The ingestion service stamps `blocks.insert_time` with
  `Utc::now()` at insert (`ingestion/src/web_ingestion_service.rs:350`). Once an insert-time bucket
  is in the past, no new block can land in it, apart from commit lag.

## Design

### 1. The ladder and rung selection (new `lakehouse/partition_ladder.rs`)

```rust
pub const LADDER: [TimeDelta; 5] = [1 min, 5 min, 1 h, 6 h, 1 day];
pub const TARGET_BUCKETS_PER_QUERY: i64 = 24;

/// Largest rung r with range / r >= TARGET_BUCKETS_PER_QUERY; LADDER[0] if none.
pub fn select_rung(query_range: TimeRange) -> TimeDelta;
pub fn finer_rung(rung: TimeDelta) -> Option<TimeDelta>;
```

Every rung divides the next one, and `duration_trunc` aligns to the Unix epoch (so days align to
UTC midnight). Buckets therefore nest and never straddle. Examples: 1 min, 15 min and 1 h → minute
rung; 6 h → 5 min; 24 h → hour (24 buckets); 7 d → 6 h; 30 d → day.

### 2. Bucket planning: open edge and cardinality cap

```rust
pub struct PlannedBucket { pub range: TimeRange /* half-open, aligned */, pub rung: TimeDelta,
                           pub closed: bool, pub nb_blocks: i64, pub nb_objects: i64 }

pub fn plan_buckets(insert_range: TimeRange, rung: TimeDelta, now: DateTime<Utc>,
                    grace: TimeDelta, minute_counts: &[(DateTime<Utc>, i64 /*blocks*/, i64 /*objects*/)],
                    max_nb_objects: i64) -> Vec<PlannedBucket>;
```

The planner walks the `rung`-aligned buckets of `insert_range` and handles each bucket `B`:

- `B` is **closed** when `B.end <= now - grace`. `grace` (default 90 s, a new
  `JitPartitionConfig` field) absorbs commit lag between `Utc::now()` at ingestion and the row
  becoming visible. It matches the maintenance daemon, whose minute task runs 30 s after a minute
  closes over the minute before it.
- If `B` is closed and its object count is at most `max_nb_objects`, emit it at `rung`.
- Otherwise, if a finer rung exists, recurse into `B`'s sub-buckets.
- Otherwise (`B` is at the minute rung), emit it as is. `group_blocks_into_partitions` keeps its
  size cuts within that minute, as it does within the hour today.
- Skip empty buckets.

As a result, the open edge is only ever materialized at the minute rung, and every partition
coarser than a minute was written while its bucket was closed. The coverage rule in §4 relies on
this invariant.

`minute_counts` comes from `fetch_bucket_counts`, extended to return `SUM(nb_objects)` next to
`COUNT(*)`. It runs over the whole insert range at the finest rung the planner can emit (the fixed
slice for `Fixed`, the minute for `RangeDriven`), and the planner sums those counts up to coarser
rungs in memory. The query returns one row per non-empty minute (at most about 43k rows for a busy
30-day process). The existing shortcut that skips the query is kept for an insert range that is a
single bucket at that resolution, so `Fixed(1 h)` skips it exactly where today's code does. When
the count query is skipped, the planner emits that single bucket as non-empty without consulting
counts, since it cannot be over the cap or need descent.

`batch_windows` and `split_into_buckets` take `&[PlannedBucket]` (or their ranges) instead of a
fixed `slice`. Batch edges stay on bucket edges, so the batching remains invisible to the specs
it produces.

### 3. Sizing mode in `JitPartitionConfig`

Replace `max_insert_time_slice: TimeDelta` with:

```rust
pub enum BucketSizing { Fixed(TimeDelta), RangeDriven { grace: TimeDelta } }
```

`Fixed(1 h)` reproduces today's behavior through the same planner (all buckets at one rung, no
cap descent, every bucket treated as closed). The two EventTime views (`thread_spans`,
`net_spans`) and their tests stay on `Fixed(1 h)`. Their cut points move between runs, and they
build cross-block trees that finer buckets would cut more often. They are out of scope.

`generate_process_jit_partitions` returns
`Vec<JitBucket { planned: PlannedBucket, specs: Vec<SourceDataBlocksInMemory> }>`. EventTime
callers flatten the result. `generate_stream_jit_partitions` keeps its return type and only
switches to `BucketSizing::Fixed`: it takes the slice from `Fixed(d)` and bails with an error on
`RangeDriven` (no caller reaches it; every caller is pinned to `Fixed(1 h)`).

### 4. Bucket-coverage freshness (replaces the InsertTime arm)

With mixed rungs, the overlap/`>=` test is unsafe. Suppose a coarse partition was written up to
14:20:00 and a later minute spec covers 14:20:00–14:21. The two overlap, and the coarse
partition's count is larger than the minute spec's, so the test calls the minute spec up to date
and the new blocks never get materialized. The InsertTime path therefore judges each planned
bucket `B` against the candidate rows `C` that intersect `B`'s half-open range:

- `inner` = rows contained in `B`; `outer` = the rest of `C`.
- **Covered** if either:
  - `outer` is empty, `Σ inner counts == Σ spec counts` (exact), and every schema hash is current.
    This is the fine-or-equal case.
  - `inner` is empty, there is one `outer` row, it contains the hull of `B`'s specs, its schema
    hash is current, and its count is `>=` the specs'. This is the coarser-rung case. It is sound
    because of the closed-when-written invariant in §2.
- **Stale** otherwise. The bucket's specs are written with `RetireMatch::Containment` when `outer`
  is empty and with `RetireMatch::Overlap` when it is not. `outer` is always taken from the raw
  (not doom-filtered) candidate set, including for buckets that become stale only through the
  cascade below, so any bucket with an outer row in the raw set uses `Overlap`. Otherwise a bucket
  beside a legacy straddling row would get `Containment`, hit 23P01 against the still-present row,
  and end up empty for the triggering query. The `Overlap` case covers a straddling
  or stale coarse row: legacy hour partitions written open before this change, or the loser of a
  race. `Overlap` passes `same_run_ranges` as it does for the EventTime views.

Retiring an `outer` row through `Overlap` can strip coverage from other buckets that relied on that
row in the same run. `resolve_bucket_coverage` (a pure function that replaces
`resolve_up_to_date_fixpoint` for InsertTime views) iterates to a fixpoint:

1. Mark as doomed every row a stale bucket's write would retire: rows inside its spec hull for
   `Containment`, rows intersecting it for `Overlap`.
2. Re-judge the buckets with doomed rows removed.
3. Repeat until no verdict changes.

Verdicts only move from covered to stale, so the loop terminates in at most `buckets.len()`
passes. The doomed set ensures the triggering query never reads a bucket that its own writes just
emptied. `spec_is_up_to_date` stays as it is for the EventTime path.

**Concurrent mixed-rung writers.** Query A writes a closed hour while query B writes a minute
inside it. Whichever commits second trips 23P01. This race already exists rarely today on the open
hour, and mixed rungs make it more frequent. On a 23P01 from a mandatory write, the shared helper
re-fetches the candidates for that bucket and re-runs the coverage rule once. If the bucket is now
covered, the write is dropped. If it is still uncovered, the write is re-attempted once with the
retire mode re-derived from the re-fetched rows (a concurrent writer that committed after the first
retire SELECT is then seen and retired), and the error propagates only if that second attempt
fails. The retry decision is a pure function over the re-fetched rows. The helper detects the
conflict with `err.chain().find_map(|e| e.downcast_ref::<PartitionOverlapConflict>())`, a typed
error that `insert_partition_transaction` returns for a `lakehouse_partitions_no_overlap`
violation (Phase 2 step 5).

### 5. Shared InsertTime `jit_update` helper

Add `jit_partitions::update_insert_time_jit_view(lakehouse, view: Arc<dyn View>, process,
query_range, stream_tag, block_processors)`. It takes an owned `Arc` because promotion passes it
to `create_merged_partition`, which takes `Arc<dyn View>`. The five views derive `Clone` and each
`jit_update` passes `Arc::new(self.clone())`. It runs the steps below, and the five views'
`jit_update` bodies collapse into a call to it.

1. Select the rung with `select_rung(query_range)`.
2. Call `generate_process_jit_partitions` with `RangeDriven`. It derives the rung internally
   (`Fixed(d) => d`, `RangeDriven{..} => select_rung(*query_time_range)`). The MIN/MAX insert range it queries is truncated outward to
   that rung, replacing today's truncation to the fixed slice, so `plan_buckets` walks whole
   rung-aligned buckets.
3. Fetch the candidates in one query over the hull (`fetch_freshness_candidates`, unchanged).
4. Call `resolve_bucket_coverage`.
5. Write the stale buckets, with the 23P01 retry.
6. Call `promote_closed_buckets` (§6) and await it.

The signature is typed with `query_range: TimeRange`, already defaulted by the caller. Each view
keeps its own process lookup (`async_events_view.rs` uses `find_process_with_latest_timing`), default
range end, `global` early return (only `log_view` and `metrics_view` have one) and block-processor
construction before calling the helper. The shared part is everything from rung selection on.

### 6. On-demand promotion (new `lakehouse/jit_promotion.rs`)

**Selection** is a pure function of the planner output, the verdicts and the candidate rows. No
object storage is read:

```rust
pub fn promotion_candidates(buckets: &[JitBucket], verdicts: &[Coverage],
                            candidates: &[PartitionFreshnessRow], query_rung: TimeDelta,
                            max_nb_objects: i64)
    -> Vec<PromotionTarget { bucket: TimeRange, inputs: Vec<PartitionFreshnessRow> }>;
```

A bucket is promoted when all of the following hold:

- The planner emitted it closed, at the query's rung (`planned.rung == query_rung`, the `select_rung(query_range)` of §5 step 1),
  not descended. Open buckets never qualify.
- It was judged covered by at least 2 `inner` rows.
- The sum of their counts is at most `max_nb_objects`.
- They all share the current schema hash.

**Execution: merge, then query.** `promote_closed_buckets` runs inline, before the query fetches
its partitions. The query then reads the coarse partitions, and later queries reuse them. The work
is optional: it never fails the query, and on contention it skips instead of waiting.

1. For each target in turn:
   - Open a transaction and call `pg_try_advisory_xact_lock(key)`, where the key hashes
     `("promote", view, instance, bucket.begin, bucket.end)`. If the lock is not granted, skip the
     target: log `info!` "skipped_locked" and record
     `imetric!("jit_promotion", "count", reason = "locked", 1_u64)`. Another query is
     already promoting it, and this one reads the fine partitions as they are.
   - Under the lock, run `SHOW transaction_read_only` on the lock transaction and pass the result
     to `micromegas_ingestion::data_lake_connection::is_read_only`. If it is on, skip with
     `reason = "read_only"` before any merge or upload. The `is_read_only_violation` check below
     stays as a fallback.
   - Re-fetch the rows in the bucket with the new inclusive-bounds single-view fetch in
     `partition_cache.rs` (Phase 3 step 7), which returns full `Partition` rows
     (`fetch_overlapping_insert_range_for_view` has strict bounds,
     while the snapshot, the merge retire and `filter_inside_range` are inclusive). Map each
     `Partition` to a `PartitionFreshnessRow` (begin/end insert time, `file_schema_hash`,
     `source_data_hash`), re-check the selection predicate, and compute the hull from the
     re-fetched set. Merge only if that hull equals the re-checked inputs' hull. Another query may
     have promoted the bucket between the snapshot and the lock.
   - The lock transaction stays open across `create_merged_partition`, which opens a second
     transaction from the same pool in `insert_partition_transaction`.
   - Call `create_merged_partition` with `insert_range` set to that hull, the re-fetched cache
     passed as both `partitions_to_merge` and `partitions_all_views`, and
     `Arc::new(ResponseWriter::new(None))` as the logger. A JIT partition's range is block-derived,
     not bucket-aligned, and the hull keeps it that way. The merge writes the sum of the inputs'
     `source_data_hash` and retires the inputs with `Containment`, so they go away atomically.
   - On success, log `info!` "promoted" and record `jit_promotion` with `reason = "promoted"`.
2. A target's failure is caught, not returned: log `warn!`, record
   `imetric!("jit_promotion_failed", "count", 1_u64)`, and move on to the next target. On a
   read-only violation (`is_read_only_violation`), log at `debug!` and record
   `jit_promotion` with `reason = "read_only"` instead. The fine partitions are untouched
   (the merge's retire and insert share one transaction), so the query still reads a complete set.

Retired files go through `temporary_files` with a one-hour expiry (`write_partition.rs:30`), so a
concurrent reader still holding the old partition list keeps reading them, as with daemon merges.

## Implementation Steps

Phase 1 steps 2-3 and Phase 2 land together as one compile unit; the five views are not flattened
as a stopgap.

### Phase 1: Ladder and planner (pure)
1. Create `rust/analytics/src/lakehouse/partition_ladder.rs` with `LADDER`,
   `TARGET_BUCKETS_PER_QUERY`, `select_rung`, `finer_rung`, `PlannedBucket` and `plan_buckets`.
   Register it in `lakehouse/mod.rs`.
2. In `jit_partitions.rs`:
   - Replace `max_insert_time_slice` with `BucketSizing`, add `grace` to `RangeDriven`, and update
     `Default` to `RangeDriven { grace: 90 s }`. The rung is derived inside
     `generate_process_jit_partitions` from the sizing and `query_time_range`.
   - Extend `fetch_bucket_counts` with `SUM(nb_objects)` and run it at the finest resolution the
     planner can emit (§2).
   - Generalize `batch_windows` and `split_into_buckets` to planned buckets.
   - Make `generate_process_jit_partitions` return `Vec<JitBucket>`.
3. Pin `thread_spans_view.rs`, `net_spans_view.rs` and their tests (including those calling
   `generate_stream_jit_partitions`) to `BucketSizing::Fixed(1 h)` (both use `..Default::default()` at `net_spans_view.rs:359-362` /
   `thread_spans_view.rs:386-389`, so the compiler won't flag them) and flatten the results of
   `generate_process_jit_partitions`.

### Phase 2: Coverage
4. In `jit_partitions.rs`, add `Coverage` and `resolve_bucket_coverage` (§4), plus the pure
   23P01-retry decision. Remove `resolve_up_to_date_fixpoint`, `find_up_to_date_partitions` (used
   only by the five views being switched) and the InsertTime arms of `spec_is_up_to_date` once
   nothing calls them. The `block_order` parameter of `spec_is_up_to_date` and
   `is_jit_partition_up_to_date` is then always `EventTime`, so remove it. In
   `rust/analytics/tests/jit_freshness_tests.rs`, delete the InsertTime-only and fixpoint tests
   (their scenarios move to `jit_bucket_coverage_tests.rs`) and cut the two tests that loop over
   both block orders to their EventTime arm.
5. Add `retire_match` and `same_run_ranges` parameters to `write_partition_from_blocks` and the
   matching fields to `BlockPartitionSpec` (`block_partition_spec.rs`), which today hard-codes
   `RetireMatch::Containment` and no `same_run_ranges`.
   The global batch specs built in `log_view.rs` and `metrics_view.rs` set
   `retire_match: RetireMatch::Containment` and an empty `same_run_ranges`, preserving today's
   behavior.
   Also change `insert_partition_transaction` in `write_partition.rs` to return a typed
   `thiserror` error `PartitionOverlapConflict` for a `lakehouse_partitions_no_overlap` violation
   in place of the string `bail!`, keeping the legible message as its `Display`.
6. Add `update_insert_time_jit_view` (§5, taking `Arc<dyn View>`), which accumulates
   `same_run_ranges` the way `net_spans_view.rs:256-293` does; derive `Clone` on the five
   InsertTime views; switch `log_view.rs`, `metrics_view.rs`,
   `async_events_view.rs`, `images_view.rs` and `otel/spans_view.rs` to it.

### Phase 3: Promotion
7. Create `rust/analytics/src/lakehouse/jit_promotion.rs` with `PromotionTarget`,
   `promotion_candidates` and `promote_closed_buckets` (§6). In `partition_cache.rs`, add an
   inclusive-bounds single-view variant of `fetch_overlapping_insert_range_for_view`
   (`begin_insert_time <= end AND end_insert_time >= begin`) sharing its row-to-`Partition`
   decoding; the promotion cache is `PartitionCache::empty(hull)` with `partitions` set.
8. Await `promote_closed_buckets` at the end of `update_insert_time_jit_view`.

### Phase 4: Docs and changelog
9. Update `mkdocs/docs/architecture/index.md` and `CHANGELOG.md` (see Documentation).

## Files to Modify

- `rust/analytics/src/lakehouse/partition_ladder.rs` (new)
- `rust/analytics/src/lakehouse/jit_promotion.rs` (new)
- `rust/analytics/src/lakehouse/mod.rs`
- `rust/analytics/src/lakehouse/jit_partitions.rs`
- `rust/analytics/src/lakehouse/block_partition_spec.rs`
- `rust/analytics/src/lakehouse/partition_cache.rs` (inclusive-bounds fetch variant)
- `rust/analytics/src/lakehouse/write_partition.rs` (typed overlap-conflict error, doc comments)
- `rust/analytics/src/lakehouse/{log_view,metrics_view,async_events_view,images_view}.rs`
  (derive `Clone`, call the helper)
- `rust/analytics/src/lakehouse/otel/spans_view.rs` (same)
- `rust/analytics/src/lakehouse/{thread_spans_view,net_spans_view}.rs` (config only)
- `rust/analytics/tests/jit_batch_windows_tests.rs`,
  `jit_partition_grouping_tests.rs`, `jit_process_batch_db_test.rs`,
  `thread_spans_ordering_db_test.rs` (config and signature updates)
- `rust/analytics/tests/jit_freshness_tests.rs` (InsertTime-only and fixpoint tests deleted, the
  two both-order tests cut to EventTime, `block_order` argument dropped)
- `rust/analytics/tests/partition_ladder_tests.rs`, `jit_bucket_coverage_tests.rs`,
  `jit_promotion_tests.rs` (new)
- `mkdocs/docs/architecture/index.md`, `CHANGELOG.md`

## Trade-offs

- **Rung = range ÷ 24 rather than the largest rung that fits.** A rolling 24 h window contains at
  most one complete aligned day. Choosing the day rung would either write a partial day or promote
  nothing. Dividing lands it on the hour rung and keeps the number of partitions per query roughly
  constant.
- **Descending to the minute rung at the open edge rather than writing the open bucket at the
  query's rung.** Writing it at the query's rung would recreate the overlap/`>=` hole from §4 and
  would make a dashboard's open minute pay for someone else's day query. Descending keeps every
  partition coarser than a minute closed, which is what makes the coarse coverage case sound.
- **Merging parquet rather than re-decoding blocks to promote.** A merge reads a few small files
  instead of decoding every block again, and `create_merged_partition` already exists with the
  right retire semantics.
- **Merge, then query, rather than background promotion.** A background task would keep the merge
  latency off the triggering query, but it needs a spawned task, a concurrency bound and shutdown
  handling. The inline merge costs the triggering query one extra read of the fine partitions and
  one write, once per bucket. Only the query that wins the try-lock pays it, so concurrent queries
  never wait behind it. Serving the query from the merge stream (a tee in
  `QueryMerger::execute_merge_query`) would remove the re-read but needs `QueryMerger` changes, so
  it is a follow-up.
- **Objects as the cap unit rather than rows or bytes.** `max_nb_objects` is the unit JIT
  partitions are already cut by, and objects are known before decoding. A size cut in a coarse bucket could fall mid-hour and break nesting. Measured `num_rows` and
  `file_size` matter for #1495's projections and joins, not for block-derived views.

## Decisions

- First cut covers the five InsertTime JIT views. The #1495 integration reuses `partition_ladder`,
  `resolve_bucket_coverage` and `jit_promotion` once that issue lands.
- The coarser-rung coverage case keeps a `>=` count check rather than an exact one. Accepted risk:
  a block committed more than `grace` after its insert-time stamp, inside a bucket already promoted
  or written coarse, would be missed. This is the same class of risk as today's `>=` test.
- `TARGET_BUCKETS_PER_QUERY` = 24 for now; it can be tuned later.
- Promotion runs inline before the query reads (merge, then query), chosen over background
  compaction.
- Promotion holds 2 pool connections per promoting query (default pool of 10, 30 s acquire timeout). Accepted risk: exhaustion is bounded by the acquire timeout and caught as `jit_promotion_failed`.
- No live-DB test is added. No bug in this area has been seen in the wild, so the DB-facing pieces
  (try-lock, merge, 23P01 retry plumbing) are covered by manual verification.

## Documentation

- Doc comments that say only the EventTime views use `RetireMatch::Overlap` or per-bucket
  `COUNT(*)`. In `write_partition.rs`: the `RetireMatch::Containment` docs ("Used by everything
  except the two EventTime JIT views"), the `RetireMatch::Overlap` docs, the `retire_partitions`
  comment, the advisory-lock comment in `insert_partition_transaction` and the
  `write_partition_from_rows` doc. In `jit_partitions.rs`: the `generate_*_jit_partitions` docs on
  per-bucket COUNT(*) and the `spec_is_up_to_date` docs.
- `mkdocs/docs/architecture/index.md`, "JIT ETL (On-Demand)": add that partition size follows the
  query range on a minute → day ladder, that only the open minute is rewritten while live, and that
  closed history is merged on demand when a longer query reads it.
- `CHANGELOG.md` (Unreleased): describe the behavior change and say that the SQL surface is
  unchanged. Add **Minor breaking change:** `JitPartitionConfig::max_insert_time_slice` is replaced
  by `bucket_sizing: BucketSizing`, `generate_process_jit_partitions` returns `Vec<JitBucket>`,
  `batch_windows` takes planned buckets, `resolve_up_to_date_fixpoint` and
  `find_up_to_date_partitions` are removed, and `spec_is_up_to_date` and
  `is_jit_partition_up_to_date` lose their `block_order` parameter, `write_partition_from_blocks`
  gains `retire_match` and `same_run_ranges` parameters, and `BlockPartitionSpec` gains
  `retire_match` and `same_run_ranges` fields.

## Testing Strategy

No-DB unit tests under `rust/analytics/tests/`:

- `partition_ladder_tests.rs`:
  - `select_rung` at each threshold (1 min, 1 h, 6 h, 24 h, 7 d, 30 d, plus sub-minute and
    multi-month ranges).
  - `plan_buckets`:
    - a rolling 24 h range produces closed hour buckets plus a tail that descends to minute
      buckets, and only the minute buckets are open;
    - a bucket over the cap descends exactly one level at a time;
    - an over-cap minute bucket is emitted as is;
    - empty buckets are skipped;
    - a single-bucket range whose count query is skipped (empty counts) yields that bucket as
      non-empty;
    - every emitted bucket nests inside one bucket of every coarser rung;
    - `Fixed(1 h)` reproduces today's buckets.
- `jit_batch_windows_tests.rs`: the existing cases ported to planned buckets, plus mixed-width
  buckets.
- `jit_bucket_coverage_tests.rs`, table-driven over `resolve_bucket_coverage`:
  - inner exact sum → covered;
  - inner sum off by one → stale (`Containment`);
  - one coarse outer row containing the hull → covered;
  - a coarse outer row ending mid-bucket (the legacy open-hour partition) → stale (`Overlap`);
  - a schema-hash mismatch → stale;
  - the doomed-row cascade, where an `Overlap` retire makes earlier buckets stale in the same run
    so no bucket is left without coverage;
  - a bucket beside a legacy straddling row gets `Overlap` (raw candidate set), not `Containment`;
  - the 23P01 retry decision, with the bucket now covered and still uncovered (second attempt
    with the retire mode re-derived from the re-fetched rows).
- `jit_promotion_tests.rs`:
  - `promotion_candidates` rejects open buckets, descended buckets, single-row buckets, over-cap
    sums and mixed schema hashes;
  - it accepts 60 minute rows inside a closed hour on an hour-rung query.
- Existing `jit_partition_grouping_tests.rs` and the EventTime cases of `jit_freshness_tests.rs`
  keep passing after the config and signature changes.

## Manual Verification

These checks need the real Postgres lock, merge and object-store round trip. Run them
with `python3 local_test_env/ai_scripts/start_services.py` and a live instrumented process
emitting logs.

1. Run `micromegas-query "SELECT count(*) FROM view_instance('log_entries','<pid>')" --begin 1m`
   several times over a few minutes. Then run
   `micromegas-query "SELECT begin_insert_time, end_insert_time, updated FROM list_partitions() WHERE view_set_name='log_entries' AND view_instance_id='<pid>' ORDER BY begin_insert_time"`.
   Expected: minute-sized partitions, and earlier minutes are not rewritten (`updated` stays the
   same).
2. Pick a closed hour `H` and record its count over a 1-hour range (`--begin <H> --end <H+1h>`).
   That range is on the minute rung, so it does not promote. Then run the same count with
   `--begin 24h`, and run the step 1 `list_partitions()` command right after. Expected: the closed
   hours have collapsed to one partition each, the open minute is still a minute partition, and
   re-running the 1-hour count for `H` gives the same number as before.
3. Fire two `--begin 24h` queries at once. Expected: both succeed, there is at most one promotion
   per bucket (the other query logs `skipped_locked` or finds no candidate), and
   `/tmp/analytics.log` shows no 23P01 errors.
