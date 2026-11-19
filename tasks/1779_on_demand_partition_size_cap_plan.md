# On-Demand Views: Bound Partition Size by Source Data Plan

GitHub issue: #1779

## Overview

On-demand materialized views (#1778) size their partitions from the query range alone: about 24
buckets on the minute / 5 min / 1 h / 6 h / 1 day ladder. A 1-day query writes 1-hour partitions,
whatever the data volume. For a dense source, one bucket's extract can then read far more than can
be processed in one step. This plan adds a mandatory, per-view, customizable byte cap,
`max_jit_src_data` (default `'1 GB'`), on how much data a single materialization step may read.
An extract reads source partitions, so a stale bucket whose source bytes exceed the cap is written
as finer ladder buckets. A promotion reads the view's own partitions, so closed partitions are
merged only while their combined size stays within the cap. Density then drives the size both
ways: dense data stays fine, sparse data rises to the range's rung, and large partitions are never
merged past the cap. The shared ladder also gains 15-minute and 30-minute rungs, so that a bucket
over the cap descends by 3× or 2× instead of jumping 12× from one hour to five minutes.

## Current State

- The ladder (`partition_ladder.rs:12`) is `[1m, 5m, 1h, 6h, 1d]`. It is shared by on-demand
  views and the block-driven JIT views, through `select_rung` (`jit_partitions.rs:133`,
  `on_demand_jit.rs:326`). Tests name rungs by position (`partition_ladder_tests.rs:22-26`,
  `on_demand_jit_tests.rs:29-30`, for example `HOUR = LADDER[2]`).
- `update_on_demand_view` (`rust/analytics/src/lakehouse/on_demand_jit.rs:323`) picks a rung with
  `select_rung(query_range)`, plans buckets with `plan_dense_buckets` (no size-driven descent; only
  the open edge descends, to 1 minute), grows them into `Unit`s around existing rows
  (`plan_units`, `settle_units`), snapshots sources over the unit hull (`snapshot_sources`), then
  probes each unit with the authored `count_src_query` (`probe_unit`, `judge_unit`) and writes the
  stale ones.
- Promotion: `promotion_targets` (`on_demand_jit.rs:149`) keeps units that are covered, closed, and
  exactly one planned bucket at the query's rung, with at least two rows. `is_promotable`
  (`jit_promotion.rs:35`) checks row count, schema hash and an object-count cap, which
  `update_on_demand_view` sets to `i64::MAX` (`on_demand_jit.rs:163`, `:397`), so on-demand merges
  are unbounded.
- `PartitionFreshnessRow` (`jit_partitions.rs:1073`) carries insert range, `file_schema_hash` and
  `source_data_hash`, but no `file_size`. It is filled by `fetch_freshness_candidates`
  (`jit_partitions.rs:1083`) and by `freshness_row` in `jit_promotion.rs`.
- The source snapshot (`PartitionCache::fetch_overlapping_insert_range`,
  `partition_cache.rs:60`) already loads every overlapping partition of every view, with
  `file_size`.
- Source detection: `inspect_scan` (`view_definition.rs:325`) walks every `MaterializedView` the
  extract scans, but records only global on-demand views. `validate_view_definition`
  (`view_definition.rs:431`) returns them as `Vec<Arc<dyn View>>`. They are stored on
  `SqlBatchView` through `with_on_demand_sources` (`sql_batch_view.rs:176`, wired in
  `view_registry.rs:209`; `flight_sql_service_impl.rs:1168` calls `validate_view_definition` and
  discards the result). Daemon-maintained sources such as
  `measures` are walked but not recorded.
- DDL options (`view_ddl.rs:172`) reject `source_partition_delta` / `merge_partition_delta` on
  on-demand views, as a v1 stopgap pending this issue. `ViewOptions` (`view_definition.rs:41`) is
  stored as JSON. `description` already shows the pattern for an optional field
  (`#[serde(default, skip_serializing_if = "Option::is_none")]`).
- There is no byte-size parser in the codebase. `parse_time_delta` (`view_definition.rs:76`) is
  the `'<n> <unit>'` precedent.
- #1778 is still under `## Unreleased` in `CHANGELOG.md`, so on-demand views have no released
  behavior to stay compatible with.

## Design

### 1. Ladder rungs at 15 and 30 minutes

`LADDER` becomes `[1m, 5m, 15m, 30m, 1h, 6h, 1d]`. Each rung still divides the next one, and
`duration_trunc` aligns them to the epoch, so buckets still nest and `RetireMatch::Containment`
still holds. Partitions already written at the old rungs nest inside the new ones, and coverage is
judged on summed counts, so no migration is needed.

Because `select_rung` is shared, the change also affects the block-driven JIT views. With the
24-bucket target, queries from 6 hours up to 12 hours move from five-minute buckets (72 for 6
hours) to 15-minute ones (24), and queries from 12 hours up to 24 hours move from five-minute
buckets (144 for 12 hours) to 30-minute ones (24 to 48). Other ranges are unchanged
(1 day → 1 h, 7 days → 6 h). The result is fewer, larger partitions and fewer probes for medium
ranges. This is a partition-layout change, not a SQL break. Descent (`plan_bucket`) and the open
edge walk more rungs, but they are pure computations over counts that were already fetched.

### 2. Option `max_jit_src_data`

- Syntax: `max_jit_src_data = '<n> <unit>'`, where the unit is `B`, `KB`, `MB` or `GB` (case
  insensitive, base 1024), mirroring `parse_time_delta`. It is parsed by a new
  `parse_byte_size(&str) -> Result<i64>` in `view_definition.rs`, which rejects non-positive
  values.
- On-demand only: a separate `update_group.is_some()` check in `view_ddl.rs` rejects it (the
  delta-rejection loop, `view_ddl.rs:237-249`, runs only when `update_group.is_none()`), plus a new
  `"max_jit_src_data"` arm in the option key match (`view_ddl.rs:186-233`).
- Stored as `ViewOptions::max_jit_src_data: Option<String>` with `serde(default,
  skip_serializing_if = "Option::is_none")`. `None` means the built-in default
  `DEFAULT_MAX_JIT_SRC_DATA = 1 GiB`, resolved in `build_sql_batch_view`.
- `SqlBatchView` gains `max_jit_src_bytes: i64`, passed to `update_on_demand_view` from
  `jit_update`.

### 3. Source views

`inspect_scan` records every scanned `MaterializedView`, deduplicated by (view set, instance).
`validate_view_definition` returns that full list. `SqlBatchView::with_on_demand_sources` becomes
`with_sources`. The on-demand subset (`requires_query_range()` and instance `"global"`, the
current rule) is derived from it where `snapshot_sources` needs it, and the daemon-reads-on-demand
check (`view_definition.rs:557-563`) filters to that same subset, so a daemon view reading
`measures` or `log_entries` is still accepted. The two existing validation tests
(including `view_reading_only_daemon_views_has_no_on_demand_sources`) and the doc comment at
`view_definition.rs:429` are updated to the new return contract. The full `ViewMetadata` from
`source.get_meta()` (set, instance and `file_schema_hash`) is kept for the byte sum.

### 4. Source bytes of a range

A pure function in `on_demand_jit.rs`:

```rust
pub fn source_bytes(partitions: &[Partition], sources: &[ViewMetadata], range: TimeRange) -> i64
```

It sums `file_size` over the source partitions, each prorated by
`|insert range ∩ range| / |insert range|`. A zero-width partition counts whole when its insert
time is in `[begin, end)`. Partitions that don't match a source's full `ViewMetadata` (including `file_schema_hash`) are
ignored, so old-schema partitions of a source view and partitions of other views don't count. Prorating assumes data is spread
evenly in insert time. That is good enough for a cap: extracts filter sources by insert time, and
the daemon's source partitions are insert-time aligned. Sources that aren't lakehouse partitions
count as 0, which keeps today's behavior for them.

### 5. Write descent for a stale unit

After `probe_unit` returns `Stale` for a closed unit planned as a single bucket
(`planned_rung = Some(r)`):

1. If `source_bytes(unit.range) <= cap`, or `r` is the finest rung, write the unit as today. At
   the finest rung with bytes over the cap, log it and record
   `on_demand_jit{outcome="over_cap"}` in addition to the write outcome
   (`written` / `conflict_dropped`).
2. Otherwise, plan the leaves with a new pure `plan_by_source_bytes(range, rung, bytes_fn, cap) ->
   Vec<PlannedBucket>` in `partition_ladder.rs`. It is a recursion shaped like `plan_bucket`: a
   bucket at or under the cap is emitted at its rung, and a bucket over the cap splits into
   `finer_rung` children, down to `LADDER[0]`. Picking leaves from source bytes alone means a dense
   first query doesn't probe every intermediate rung.
3. Run `plan_units(leaves, rows)`. If that yields exactly the original unit (existing coarse rows
   force it back together), write the whole unit and record `over_cap` in addition to the write outcome. Otherwise probe each
   sub-unit, and write it when it is stale, with the same conflict handling (`resolve_conflict`).
   Sub-units don't descend again: their leaves were already sized.

Grown units (`planned_rung = None`) and open units are written as today. Open units are already at
1 minute. Grown units only exist around earlier rows, which were written under the cap unless they
hit the floor or the `over_cap` fallback.

Descent stays inside the unit, so the snapshot taken over the unit hull already covers every leaf.
Neither the snapshot range nor the order of `update_on_demand_view` changes.

### 6. Promotion within the cap

- `PartitionFreshnessRow` gains `file_size: i64`. `fetch_freshness_candidates` selects it and
  `freshness_row` copies it.
- `is_promotable(inputs, max_nb_objects, max_bytes)` also requires `Σ file_size <= max_bytes`. The
  block-driven JIT path passes `i64::MAX`, which leaves it unchanged, and so does
  `promotion_candidates` (`jit_promotion.rs:76`). `promote_closed_buckets` and
  `promote_one` take `max_bytes`, and the lock-time recheck applies it.
- `promotion_targets(units, rows, verdicts, cap)` keeps covered, closed, ungrown units at any
  planned rung, not only the query's rung, because descended leaves can be covered too. For each
  one it calls a new pure `split_promotion(range, rung, rows, cap) -> Vec<PromotionTarget>`:
  - If the rows are promotable within the cap, the target is the whole range.
  - Otherwise, if their total exceeds the cap and a finer rung exists, split the range into child
    buckets and recurse into each one with the rows lying inside it. A child that a row straddles
    is skipped, since it can't be split.
  - With one row or none there is nothing to merge.

  A covered parent proves only the summed count, not each child's count. That is harmless:
  merging keeps both the data and the summed `source_data_hash`, so a stale child stays detectably
  stale.
- Rows written by the current query carry a `Stale` verdict and are not promoted until a later
  query, as today.

### Resulting behavior (`select * from measures`, cap 1 GB)

A dense deployment can write about 1.5 GB of `measures` partitions per hour, and about 2.4 GB at
peak.

- First 1-day query: 24 hourly probes, all stale. A typical hour is over the cap, so its leaves
  are 30-minute buckets of about 770 MB. A peak hour descends to 15-minute buckets of about
  600 MB. That is 48 to 96 writes per day, against 288 with the old ladder at the same cap.
- A repeated query: 24 probes, all covered. Promotion finds 1.5 GB or more per hour, which is over
  the cap, so it recurses to 30 minutes, finds a single row per bucket, and merges nothing.
- A `log_entries`-sized source (about 460 MB/h) is written in 1-hour buckets, and never merged
  into 6 hours (about 2.7 GB).
- A sparse view: buckets stay at the range's rung, and later longer-range queries merge them up
  as long as the merged result fits under the cap.

## Implementation Steps

1. **Ladder**: insert the 15m and 30m rungs into `LADDER` in `partition_ladder.rs`. In the
   tests, replace the positional rung constants with explicit `TimeDelta`s first. After the
   insert, `LADDER[2]` silently becomes 15 minutes, and the compiler won't flag it. Then update
   on purpose the assertions that hard-code a rung choice: the 6-hour and 23-hour assertions
   (`partition_ladder_tests.rs:36-37`, now 15m and 30m). The 2-hour one stays at 5m. Update the
   `partition_ladder.rs` module doc comment (line 1 lists "minute, 5 min, hour, 6 h, day").
2. **Option parsing**: add `parse_byte_size` and `DEFAULT_MAX_JIT_SRC_DATA` to
   `view_definition.rs`, and add the `ViewOptions::max_jit_src_data` field. Accept the option in
   `view_ddl.rs`, reject it on daemon views, and update the doc comment on
   `view_definition_from_options`.
3. **View wiring**: resolve the cap in `build_sql_batch_view` and add `max_jit_src_bytes` to
   `SqlBatchView`. Record all scanned sources in `inspect_scan` / `validate_view_definition`.
   Rename `with_on_demand_sources` to `with_sources`, update `view_registry.rs:209`, and derive
   the on-demand subset for `snapshot_sources` and for the daemon-reads-on-demand check. Update the
   two validation tests and the `view_definition.rs:429` doc comment.
   `flight_sql_service_impl.rs:1168` calls `validate_view_definition` and discards the result, so
   it needs no change beyond compiling.
4. **Freshness row**: add `file_size` to `PartitionFreshnessRow`, `fetch_freshness_candidates` and
   `freshness_row`. Fix the constructors the compiler flags, mostly in tests.
5. **Ladder planner**: add `plan_by_source_bytes` to `partition_ladder.rs`.
6. **Promotion cap**: add `max_bytes` to `is_promotable` (made `pub` so integration tests can
   reach it), `promote_one` and `promote_closed_buckets`. Pass `i64::MAX` in
   `jit_partitions.rs:1542` and in `promotion_candidates` (`jit_promotion.rs:76`).
7. **On-demand driver**: in `on_demand_jit.rs`, add `source_bytes` and `split_promotion`,
   generalize `promotion_targets`, add the stale-unit descent to the unit loop of
   `update_on_demand_view`, the `over_cap` outcome, and pass the cap to `promote_closed_buckets`.
8. **Docs and CHANGELOG** (see Documentation).
9. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test -p micromegas-analytics`.

## Files to Modify

- `rust/analytics/src/lakehouse/view_definition.rs`: `parse_byte_size`, default, `ViewOptions`
  field, `build_sql_batch_view`, `inspect_scan`, `validate_view_definition`
- `rust/analytics/src/lakehouse/view_ddl.rs`: option parsing and rejection
- `rust/analytics/src/lakehouse/sql_batch_view.rs`: `max_jit_src_bytes`, `with_sources`,
  `jit_update`
- `rust/analytics/src/lakehouse/view_registry.rs`: `with_sources`
- `rust/analytics/src/lakehouse/jit_partitions.rs`: `PartitionFreshnessRow.file_size`,
  `fetch_freshness_candidates`, the `promote_closed_buckets` call
- `rust/analytics/src/lakehouse/jit_promotion.rs`: byte cap in `is_promotable` / `promote_one` /
  `promote_closed_buckets`, `freshness_row`, `promotion_candidates` (passes `i64::MAX`)
- `rust/analytics/src/lakehouse/partition_ladder.rs`: `LADDER`, `plan_by_source_bytes`
- `rust/analytics/src/lakehouse/on_demand_jit.rs`: `source_bytes`, `split_promotion`,
  `promotion_targets`, descent in `update_on_demand_view`
- Tests: `rust/analytics/tests/{partition_ladder_tests,on_demand_jit_tests,jit_promotion_tests,view_ddl_parse_tests,view_definition_validation_tests}.rs`
- `mkdocs/docs/admin/materialized-views.md`, `mkdocs/docs/architecture/index.md`, `CHANGELOG.md`

## Trade-offs

- **One threshold, not separate write and merge caps.** A write cap smaller than the merge cap
  means writing small and merging later, which is wasted work whenever the data would fit at the
  coarser size in the first place. One cap on "bytes read in one step" covers both, because each
  step measures its own input.
- **Source bytes for writes, own partition bytes for merges**, not output bytes for both. Output
  size is unknown before the first write. Each step is capped on what it actually reads, so an
  aggregating view, with small output, can be merged much coarser than it could be extracted.
- **Probe at the query's rung, then descend from source bytes**, instead of descending from
  source bytes before probing. Descending first would make every repeated query of covered dense
  history probe each fine bucket (1,440 probes for a minute-grained day). This way covered history
  costs one probe per query-rung bucket, and only stale dense buckets pay for descent.
- **A width cap (`max_partition_delta`) or count-driven descent** (from the issue) were rejected.
  A width doesn't adapt to density that changes over time. Thresholds in the author's
  `count_src_query` units mean something different for every view. Bytes are uniform, and they
  are already in the catalog.
- **A byte estimate from the source snapshot**, rather than a `count_src_query`-based estimate:
  the snapshot is already loaded, so the estimate needs no query.

## Decisions

- The cap is mandatory, with a customizable default of 1 GiB. It is not off by default (user
  call, after weighing 256 MB, 512 MB and 2 GB).
- The ladder gains 15-minute and 30-minute rungs in this change, and the block-driven JIT views
  get them too (user call). There is no 3-hour rung until data shows a need.
- The default is resolved at load time, not written into the stored definition, so it can be
  tuned centrally.
- Distributing partition creation to cut the latency of the first dense query is out of scope.
  Units are planned, probed and written independently, which keeps that option open.
- An existing row above the cap is not split. A stale unit containing one is rewritten whole and
  counted as `over_cap`. A 1-minute bucket over the cap is written anyway: the ladder has no finer
  rung.
- `source_partition_delta` / `merge_partition_delta` stay rejected on on-demand views.
  `max_jit_src_data` is the size control for them.
- Block-driven JIT views keep their `max_nb_objects` rule. Their promotion passes no byte cap.
- No CHANGELOG "minor breaking change" clause for the on-demand behavior: #1778 is unreleased. The
  CHANGELOG bullet states the new rungs, and how they change the bucket sizes of the block-driven
  JIT views for queries from 6 hours to under 24 hours. The bullet ends with a **Minor breaking
  change:** clause for the Rust API changes (see Documentation).

## Documentation

- `mkdocs/docs/admin/materialized-views.md`:
  - Options table: a new `max_jit_src_data` row (on-demand only, default `'1 GB'`, units, and
    what it bounds), and the delta rows point to it for on-demand views.
  - "On-demand views" → "Partition sizes follow the query range": list the new rungs (1 minute,
    5 minutes, 15 minutes, 30 minutes, 1 hour, 6 hours, 1 day), and describe the descent when a
    bucket's source data exceeds the cap, the merge cap, the 1-minute floor, and that very large
    existing partitions are not split.
  - "Cost" bullet: a dense stale range costs one write per fine bucket.
- `mkdocs/docs/architecture/index.md:225`: the ladder listing gains the 15-minute and 30-minute
  rungs.
- `CHANGELOG.md`: a new Unreleased bullet for #1779. It ends with a **Minor breaking change:**
  clause listing the `validate_view_definition` return type, `with_on_demand_sources` renamed to
  `with_sources`, the new `PartitionFreshnessRow.file_size` field, and the new `max_bytes`
  argument of `is_promotable` / `promote_one` / `promote_closed_buckets`.

## Testing Strategy

All of these are no-DB unit tests over the pure functions, plus the existing offline factory
helpers:

- `view_ddl_parse_tests.rs`: `max_jit_src_data` accepted on an on-demand view, rejected with
  `update_group`, a bad unit or a non-positive value rejected, absent → `None`.
- `view_definition_validation_tests.rs` (`try_validate_sources`): an extract over a daemon
  source (e.g. `measures`) records it as a source but not as an on-demand source, and a daemon view reading it
  is still accepted. A `ViewOptions`
  JSON without the field deserializes. `parse_byte_size` cases, including case and units.
- `partition_ladder_tests.rs`: the existing "each rung divides the next" test covers the new
  rungs. `select_rung` picks 15m for a 6-hour range and 30m for a 12-hour range, and keeps 1h for
  1 day and 6h for 7 days. `plan_by_source_bytes`: sparse → stays at the rung. Uniform dense
  → all at the rung that fits. Uneven density → mixed rungs that tile the range. Over the cap at
  `LADDER[0]` → emitted at the floor.
- `on_demand_jit_tests.rs`:
  - `source_bytes`: full containment, prorated straddle, zero-width partition, other views
    ignored, stale-schema partition ignored.
  - `split_promotion`: fits → one target; over the cap → child targets; straddling row → that
    child skipped; a single row → no target.
  - `promotion_targets`: covered leaves at a finer rung qualify, grown or open units don't.
  - Descent fallback: `plan_units` over leaves that an existing coarse row forces back together
    yields the original unit.
- `jit_promotion_tests.rs`: `is_promotable` (now `pub`) rejects inputs whose `file_size` sum exceeds
  `max_bytes`, and accepts them at the cap exactly.

The async driver wiring in `update_on_demand_view` needs a live DB and object store. Per
`CONTRIBUTING.md`, it is covered by manual verification rather than a new `#[ignore]` test.

## Manual Verification

Checked by hand because the driver's wiring needs real services and ingested data:

1. Start the services: `python3 local_test_env/ai_scripts/start_services.py`. Ingest data with
   `python3 local_test_env/ai_scripts/run_generator.py` for at least about an hour, and wait for
   the hourly buckets to close before step 4.
2. Create an on-demand view over `measures` with a tiny cap so it descends:
   `micromegas-query "CREATE MATERIALIZED VIEW m_cap WITH (extract_query = \$\$SELECT * FROM measures WHERE insert_time >= '{begin}' AND insert_time < '{end}'\$\$, count_src_query = \$\$SELECT count(*) AS count FROM measures WHERE insert_time >= '{begin}' AND insert_time < '{end}'\$\$, merge_partitions_query = \$\$SELECT * FROM {source}\$\$, time_column = 'time', max_jit_src_data = '1 MB')" --all`.
   Expected: accepted. The same statement with `update_group = 10` is rejected.
3. `micromegas-query "SELECT count(*) FROM m_cap" --begin 6h` (a 15-minute rung), then
   `micromegas-query "SELECT begin_insert_time, end_insert_time, file_size FROM list_partitions() WHERE view_set_name = 'm_cap' ORDER BY begin_insert_time" --all`.
   Expected: closed partitions are narrower than the 15-minute query rung wherever a 15-minute
   bucket's source data is over 1 MB.
4. Run `DROP MATERIALIZED VIEW m_cap` (which retires its partitions), recreate the view with
   `max_jit_src_data = '1 GB'`, and re-run step 3. Expected: closed partitions at the 15-minute rung. A second query over
   `--begin 1d` (a 1-hour rung) merges closed rows into 1-hour partitions whose `file_size` stays
   under the cap.

## Follow-ups

- #1802: remove `source_partition_delta` / `merge_partition_delta` and size daemon-updated views
  with the same byte cap, and decide whether block-driven JIT views adopt it.
