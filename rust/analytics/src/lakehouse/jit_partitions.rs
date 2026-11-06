//! Just-in-time (JIT) partition generation groups a view's source blocks (queried from
//! `blocks_view` in `ORDER BY insert_time, block_id` order) into `SourceDataBlocksInMemory`
//! partitions no larger than `JitPartitionConfig::max_nb_objects`.
//!
//! `JitPartitionConfig::block_order` (see `BlockOrder`) picks between two orderings for the
//! blocks a partition ends up holding: `InsertTime` (the SQL order, unchanged) for views that
//! decode blocks independently, and `EventTime` for views that build cross-block trees or rely on
//! event-time-sorted rows within a partition file. `group_blocks_into_partitions` is the single
//! place both variants are cut, and it upholds one invariant regardless of ordering: every emitted
//! partition's `[min_insert_time, max_insert_time]` range is non-overlapping with, and
//! non-decreasing relative to, every other emitted partition's -- the precondition the
//! `lakehouse_partitions_no_overlap` exclusion constraint enforces at insert time. See
//! `group_blocks_into_partitions`'s own docs for how that invariant is upheld under
//! `BlockOrder::EventTime`, where the natural size-based cut point is not always safe, and for
//! the one (production-unreachable) shape it does not cover.
//!
//! Note that this is an *insert-time* invariant only. It says nothing about partitions'
//! *event-time* ranges, which for the block-derived views are computed from block
//! `begin_ticks`/`end_ticks`. By design both `micromegas_tracing` and the Unreal producer now stamp
//! a single shared timestamp per flush, used for both the outgoing block's close and the
//! replacement block's `begin`, so consecutive blocks touch exactly and adjacent partitions' event
//! bounds do not overlap by construction. Data from producer versions predating that fix can still
//! carry strictly-overlapping block ticks; `ThreadSpansView`'s recorded `max_sort_key_time` is what
//! makes that harmless to the `Concatenated` scan check (`partitioned_execution_plan::partition_bounds`
//! prefers it over the looser `max_event_time`) rather than requiring every source block to be
//! re-ingested. See the ordering-invariant notes on `View::get_scan_output_ordering` and
//! `tasks/completed/thread_spans_segment_boundary_overlap_plan.md`.

use super::{
    block_partition_spec::{BlockPartitionSpec, BlockProcessorMap},
    blocks_view::BlocksView,
    jit_promotion::{promote_closed_buckets, promotion_candidates},
    lakehouse_context::LakehouseContext,
    partition_cache::{LivePartitionProvider, QueryPartitionProvider},
    partition_ladder::{
        BucketCount, LADDER, PlannedBucket, align_outward, plan_buckets, select_rung,
    },
    partition_source_data::{PartitionSourceBlock, SourceDataBlocksInMemory},
    view::{View, ViewMetadata},
    write_partition::{PartitionOverlapConflict, RetireMatch},
};
use crate::{
    dfext::{
        string_column_accessor::string_column_by_name,
        typed_column::{get_single_row_primitive_value, typed_column_by_name},
    },
    lakehouse::{
        partition_cache::PartitionCache, partition_source_data::hash_to_object_count,
        query::query_partitions, view::PartitionSpec,
    },
    metadata::{
        ProcessMetadata, StreamMetadata, block_from_batch_row, stream_metadata_from_batch_row,
    },
    response_writer::ResponseWriter,
    time::TimeRange,
};
use anyhow::{Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::arrow::array::{Int64Array, TimestampNanosecondArray};
use datafusion::arrow::datatypes::{Schema, TimestampNanosecondType};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::types::block::BlockMetadata;
use micromegas_tracing::prelude::*;
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// How source blocks are ordered before being cut into JIT partitions.
///
/// The choice matters because `generate_stream_jit_partitions_segment` and the batched block
/// queries driven by `generate_stream_jit_partitions` / `generate_process_jit_partitions` fetch
/// blocks with `ORDER BY insert_time, block_id`, and that SQL order is what
/// `group_blocks_into_partitions` starts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockOrder {
    /// Registration order (`insert_time`, `block_id`), i.e. the SQL order is kept as-is. Correct
    /// for views that decode each block independently and declare no scan ordering: cutting an
    /// insert-ordered list always yields insert-time ranges that are non-decreasing and
    /// non-overlapping across partitions, by construction.
    InsertTime,
    /// Event order (`begin_ticks`, `end_ticks`). Required by views that build cross-block trees or
    /// derive event-time bounds from the list endpoints, and by any view declaring
    /// `ScanOrdering::Concatenated` over an event-time column. Sorting by event time can put blocks
    /// with out-of-order `insert_time` inside one partition or straddling a cut point, so
    /// `group_blocks_into_partitions` additionally enforces insert-safe cut points (see its docs)
    /// to keep JIT partitions' insert-time ranges non-overlapping.
    EventTime,
}

/// How insert-time buckets are sized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BucketSizing {
    /// Every bucket is the same width and treated as closed, with no descent on object count.
    Fixed(TimeDelta),
    /// The bucket width follows the query range along `partition_ladder::LADDER`. `grace` is how
    /// long after a bucket ends it is still considered open.
    RangeDriven { grace: TimeDelta },
}

/// Configuration for Just-In-Time (JIT) partition generation.
pub struct JitPartitionConfig {
    pub max_nb_objects: i64,
    pub bucket_sizing: BucketSizing,
    pub block_order: BlockOrder,
    /// Soft row-count target for a single batched block query (see `batch_windows`). Consecutive
    /// insert-time buckets are packed into one query up to this many blocks, derived from a
    /// per-bucket count query run before the batch queries themselves. A config field rather
    /// than a bare constant so DB-gated tests can lower it to force a run to split into more than
    /// one batch.
    pub target_rows_per_query: i64,
}

impl Default for JitPartitionConfig {
    fn default() -> Self {
        JitPartitionConfig {
            max_nb_objects: 20 * 1024 * 1024,
            bucket_sizing: BucketSizing::RangeDriven {
                grace: TimeDelta::seconds(90),
            },
            block_order: BlockOrder::InsertTime,
            target_rows_per_query: 250_000,
        }
    }
}

impl JitPartitionConfig {
    /// `(rung, finest)`: the width the query's buckets start at, and the narrowest the planner
    /// may descend to. Also the resolution of the per-bucket counts.
    fn rungs(&self, query_range: &TimeRange) -> (TimeDelta, TimeDelta) {
        match self.bucket_sizing {
            BucketSizing::Fixed(slice) => (slice, slice),
            BucketSizing::RangeDriven { .. } => (select_rung(*query_range), LADDER[0]),
        }
    }

    fn plan(
        &self,
        insert_range: TimeRange,
        rung: TimeDelta,
        finest: TimeDelta,
        counts: &[BucketCount],
    ) -> Vec<PlannedBucket> {
        let (now, grace) = match self.bucket_sizing {
            // Every bucket is closed under fixed sizing.
            BucketSizing::Fixed(_) => (DateTime::<Utc>::MAX_UTC, TimeDelta::zero()),
            BucketSizing::RangeDriven { grace } => (Utc::now(), grace),
        };
        plan_buckets(
            insert_range,
            rung,
            finest,
            now,
            grace,
            counts,
            self.max_nb_objects,
        )
    }
}

/// A planned bucket with the partition specs grouped from its blocks.
pub struct JitBucket {
    pub planned: PlannedBucket,
    pub specs: Vec<SourceDataBlocksInMemory>,
}

/// Returns the real `[min, max]` `insert_time` range covered by `blocks`, independent of list
/// order. Always use this instead of reading `blocks[0]`/`blocks[last]`: under
/// `BlockOrder::EventTime` the list is sorted by event time, not insert time, so its endpoints are
/// no longer the insert-time extremes.
pub fn blocks_insert_time_range(blocks: &[Arc<PartitionSourceBlock>]) -> Result<TimeRange> {
    let mut blocks_iter = blocks.iter();
    let first = blocks_iter
        .next()
        .with_context(|| "blocks_insert_time_range: empty block list")?;
    let mut min_insert_time = first.block.insert_time;
    let mut max_insert_time = first.block.insert_time;
    for block in blocks_iter {
        min_insert_time = min_insert_time.min(block.block.insert_time);
        max_insert_time = max_insert_time.max(block.block.insert_time);
    }
    Ok(TimeRange::new(min_insert_time, max_insert_time))
}

/// Splits an event-time-ordered block list into maximal chains whose tick coverage is unbroken, so
/// each chain can be decoded into one cross-block tree (a call tree for `thread_spans`, a net span
/// tree for `net_spans`).
///
/// A chain breaks only on a *gap* -- `begin_ticks` strictly after the running `end_ticks`, meaning
/// blocks are missing in between and a tree built across the seam would be nonsense. An *overlap*
/// (`begin_ticks` at or before the running end) still means unbroken coverage and keeps the chain
/// open. The chain rule stays tolerant of overlap (not just equality) deliberately: by design both
/// `micromegas_tracing::dispatch`'s flush paths and the Unreal producer now stamp one shared
/// timestamp per flush, so consecutive blocks from a healthy producer touch exactly
/// (`begin_ticks == running end_ticks`) -- but blocks from producer versions predating that fix
/// (or any other producer stamping two separate timestamps) can still carry a real, small overlap,
/// and those legacy blocks are exactly the ones this function must keep chaining correctly. An
/// equality-only test would incorrectly break the chain on every such legacy seam.
///
/// The running end is a max, not just the previous block's `end_ticks`: a chain must not be broken
/// by a short block fully contained in an earlier, longer one.
pub fn group_contiguous_block_chains(
    blocks: &[Arc<PartitionSourceBlock>],
) -> Vec<Vec<BlockMetadata>> {
    let mut chains: Vec<Vec<BlockMetadata>> = vec![];
    let mut chain: Vec<BlockMetadata> = vec![];
    let mut chain_end: Option<i64> = None;
    for block in blocks {
        match chain_end {
            // A gap: close the chain and re-seed from this block.
            Some(end) if block.block.begin_ticks > end => {
                chains.push(std::mem::take(&mut chain));
                chain_end = Some(block.block.end_ticks);
            }
            // Touching or overlapping: extend the chain.
            Some(end) => chain_end = Some(end.max(block.block.end_ticks)),
            None => chain_end = Some(block.block.end_ticks),
        }
        chain.push(block.block.clone());
    }
    if !chain.is_empty() {
        chains.push(chain);
    }
    chains
}

/// Emits one partition (if it holds any objects) from `blocks[start..end]`, logging once if the
/// window genuinely grew past `max_nb_objects` because no insert-safe cut point was available.
fn emit_partition(
    out: &mut Vec<SourceDataBlocksInMemory>,
    blocks: &[Arc<PartitionSourceBlock>],
    start: usize,
    end: usize,
    nb_objects: i64,
    grown_past_limit: i64,
    max_nb_objects: i64,
) {
    // A window whose blocks all report nb_objects == 0 is dropped rather than emitted as a
    // rows-free partition. No *in-repo* producer emits a zero-object block --
    // `micromegas_tracing::dispatch`'s flush paths skip empty streams, the Unreal sink does the
    // same, and OTLP ingestion returns no block for a zero-record batch -- but nothing enforces it
    // (`nb_objects` has no CHECK constraint and the ingestion API does not validate it). Dropping
    // is safe: such blocks carry no rows, and each partition's DB range is derived per-spec from
    // `blocks_insert_time_range`, not from full block coverage.
    if nb_objects == 0 {
        return;
    }
    if grown_past_limit > 0 {
        // process_id only: under the process-level path (`generate_process_jit_partitions`) a
        // partition can span several streams, and after the event-time sort `blocks[start]` is an
        // arbitrary one of them.
        //
        // debug!, not warn!: grouping re-runs on every query over the view (jit_update is called
        // from every scan, and the stable sort makes cut decisions deterministic), so a persistent
        // inversion would re-emit the same message on every query forever.
        let process_id = blocks[start].process.process_id;
        debug!(
            "group_blocks_into_partitions: process={process_id} emitted a partition of \
             {nb_objects} objects (soft limit max_nb_objects={max_nb_objects}) after \
             {grown_past_limit} cut(s) deferred by insert-time inversions with no insert-safe cut \
             point available"
        );
    }
    out.push(SourceDataBlocksInMemory {
        blocks: blocks[start..end].to_vec(),
        block_ids_hash: nb_objects.to_le_bytes().to_vec(),
    });
}

/// Groups a segment's source blocks into JIT partitions.
///
/// Two invariants must both hold across the returned partitions:
/// - **Size** -- `max_nb_objects` is a soft, whole-block-granularity limit (a block is never
///   split, so a single oversized block already exceeds it).
/// - **Insert-time non-overlap** -- the `lakehouse_partitions_no_overlap` exclusion constraint
///   requires each partition's `[min_insert_time, max_insert_time]` to be non-overlapping with
///   (and non-decreasing relative to) every other partition's. Adjacent partitions may *touch*
///   (one's max equals the next's min): the constraint's `tstzrange` is half-open `[)`, so that
///   is not an overlap. The one shape this does not cover is two or more partitions sharing an
///   *identical degenerate* range (every block in both carrying the same `insert_time`), which
///   requires more than `max_nb_objects` objects at a single microsecond and so does not occur in
///   production; `tstzrange(t, t)` is empty, so the constraint stays quiet, but the
///   up-to-date check (`spec_is_up_to_date`) would see more than one row for that range and never
///   report the partitions up to date.
///
/// Under `BlockOrder::InsertTime` both invariants hold for free: `blocks` is already sorted by
/// `insert_time`, so every candidate cut point is insert-safe and this function cuts at exactly
/// `max_nb_objects` -- identical to a naive greedy cut except that an all-zero-object window emits
/// nothing rather than a rows-free partition (see `emit_partition`; unreachable in practice).
///
/// Under `BlockOrder::EventTime`, `blocks` is stable-sorted by `(begin_ticks, end_ticks)` first
/// (ties keep the incoming, insert-ordered position, so grouping stays deterministic); a cut point
/// is then only taken where it is *insert-safe*: every block already in the partition being closed
/// must have an `insert_time` no later than every remaining block's, computed via a suffix-minimum
/// over the event-time-sorted list. When the natural `max_nb_objects` cut point isn't safe, the cut
/// looks back to the most recent safe index; when no safe index exists in the window, the window
/// grows past the soft limit and a `debug!` logs it.
pub fn group_blocks_into_partitions(
    config: &JitPartitionConfig,
    mut blocks: Vec<Arc<PartitionSourceBlock>>,
) -> Vec<SourceDataBlocksInMemory> {
    if config.block_order == BlockOrder::EventTime {
        // Stable sort: ties keep the incoming (insert_time, block_id) order.
        blocks.sort_by(|a, b| {
            (a.block.begin_ticks, a.block.end_ticks).cmp(&(b.block.begin_ticks, b.block.end_ticks))
        });
    }
    let n = blocks.len();
    if n == 0 {
        return vec![];
    }

    // suffix_min[i] = min(insert_time of blocks[i..n]) over the (possibly event-time-sorted)
    // list; suffix_min[n] = DateTime::<Utc>::MAX_UTC so a cut at n (the tail) is always safe.
    let mut suffix_min = vec![DateTime::<Utc>::MAX_UTC; n + 1];
    for i in (0..n).rev() {
        suffix_min[i] = suffix_min[i + 1].min(blocks[i].block.insert_time);
    }

    let mut out = vec![];
    let mut start = 0usize;
    let mut nb_objects: i64 = 0;
    let mut prefix_max_insert = DateTime::<Utc>::MIN_UTC;
    // (index, nb_objects of blocks[start..index]) of the most recent safe cut point seen since
    // the current window started.
    let mut last_safe: Option<(usize, i64)> = None;
    // Counts only the "no safe cut point exists" (grow-past-limit) events below; the look-back
    // branch logs about itself directly and does not feed this counter (its emitted prefix is
    // provably <= max_nb_objects, so it must not carry the soft-limit wording).
    let mut grown_past_limit: i64 = 0;

    let mut i = 0usize;
    while i < n {
        // A cut at i is safe iff every block already accumulated (blocks[start..i]) was
        // inserted no later than every remaining block (blocks[i..]): the emitted partition's
        // insert range then cannot overlap any later partition's.
        let safe_here = prefix_max_insert <= suffix_min[i];
        if safe_here && i > start {
            last_safe = Some((i, nb_objects));
        }
        let block_nb_objects = blocks[i].block.nb_objects as i64;
        let full = nb_objects + block_nb_objects > config.max_nb_objects && i > start;
        if full {
            if safe_here {
                // The natural cut point is insert-safe: cut here, <= max_nb_objects.
                emit_partition(
                    &mut out,
                    &blocks,
                    start,
                    i,
                    nb_objects,
                    grown_past_limit,
                    config.max_nb_objects,
                );
                start = i;
                nb_objects = 0;
                prefix_max_insert = DateTime::<Utc>::MIN_UTC;
                last_safe = None;
                grown_past_limit = 0;
                continue;
            } else if let Some((j, j_nb_objects)) = last_safe {
                // Look back to the most recent safe index: emit the already-safe prefix (which is
                // provably <= max_nb_objects, since `full` only trips once nb_objects already
                // fits under the limit), then re-seed the window from j and replay blocks[j..i)
                // to recompute running state before reprocessing block i (which may itself still
                // be unsafe -- a straggler's window is not bounded by this rule, see the module
                // docs).
                // process_id only -- see the note in `emit_partition`. debug!, not warn!: this is
                // the designed, insert-safe path and re-fires on every query over the view for as
                // long as the inversion exists in the source blocks.
                let process_id = blocks[start].process.process_id;
                debug!(
                    "group_blocks_into_partitions: process={process_id} cut moved back from index \
                     {i} to {j} ({} block(s) looked back)",
                    i - j
                );
                emit_partition(
                    &mut out,
                    &blocks,
                    start,
                    j,
                    j_nb_objects,
                    0,
                    config.max_nb_objects,
                );
                start = j;
                nb_objects = 0;
                prefix_max_insert = DateTime::<Utc>::MIN_UTC;
                last_safe = None;
                grown_past_limit = 0;
                // No last_safe recomputation while replaying: no index in (j, i] can be safe
                // after the re-seed. j was the most recent safe index, so every block before j
                // has insert_time <= suffix_min[j] <= suffix_min[m] for any m > j -- meaning a
                // cut at m is safe in the re-seeded window iff it was safe in the original one,
                // and the original pass already found all of (j, i] unsafe (otherwise last_safe
                // would have pointed there instead of j).
                for replayed in &blocks[start..i] {
                    nb_objects += replayed.block.nb_objects as i64;
                    prefix_max_insert = prefix_max_insert.max(replayed.block.insert_time);
                }
                continue;
            } else {
                // No safe cut point exists anywhere in this window: grow past the soft limit
                // rather than emit a partition whose insert range could overlap a later one.
                grown_past_limit += 1;
            }
        }
        nb_objects += block_nb_objects;
        prefix_max_insert = prefix_max_insert.max(blocks[i].block.insert_time);
        i += 1;
    }
    emit_partition(
        &mut out,
        &blocks,
        start,
        n,
        nb_objects,
        grown_past_limit,
        config.max_nb_objects,
    );
    out
}

/// Packs consecutive planned buckets into batch query windows, greedily closing a window just
/// before it would exceed `target_rows_per_query`.
///
/// A bucket whose own block count already exceeds `target_rows_per_query` still forms one batch on
/// its own -- the loop never splits a bucket.
///
/// Batch edges are always bucket edges and batches tile from the first bucket's start to the last
/// bucket's end with no gaps or overlaps (the gaps between planned buckets hold no blocks), so
/// which width is picked cannot change the specs `group_blocks_into_partitions` later emits per
/// bucket -- only how many SQL queries it takes to gather them. `buckets` must be ascending; an
/// empty `buckets` yields no window.
pub fn batch_windows(
    buckets: &[PlannedBucket],
    target_rows_per_query: i64,
) -> impl Iterator<Item = TimeRange> {
    let mut windows = Vec::new();
    let Some(first) = buckets.first() else {
        return windows.into_iter();
    };
    let mut running: i64 = 0;
    let mut batch_begin = first.range.begin;
    for bucket in buckets {
        if running > 0 && running + bucket.nb_blocks > target_rows_per_query {
            windows.push(TimeRange::new(batch_begin, bucket.range.begin));
            running = 0;
            batch_begin = bucket.range.begin;
        }
        running += bucket.nb_blocks;
    }
    if let Some(last) = buckets.last() {
        windows.push(TimeRange::new(batch_begin, last.range.end));
    }
    windows.into_iter()
}

/// Renders a `TimeDelta` as arrow-parsable interval text (e.g. `"3600 seconds"`) for use with
/// DataFusion's `date_bin`. `TimeDelta`'s own `Display` yields ISO-8601 (`PT3600S`), which
/// `date_bin` cannot parse.
fn interval_literal(slice: TimeDelta) -> String {
    format!("{} seconds", slice.num_seconds())
}

/// Distributes an insert-time-sorted block list over `buckets` (ascending, parallel to the
/// result). The SQL feeding this is `ORDER BY insert_time, block_id`, so each bucket's blocks are a
/// contiguous run. A block outside every bucket (committed between the count query and the fetch)
/// is left for the next run to plan.
fn split_into_buckets(
    blocks: Vec<Arc<PartitionSourceBlock>>,
    buckets: &[PlannedBucket],
) -> Vec<Vec<Arc<PartitionSourceBlock>>> {
    let mut split: Vec<Vec<Arc<PartitionSourceBlock>>> = vec![vec![]; buckets.len()];
    let mut idx = 0usize;
    for block in blocks {
        let insert_time = block.block.insert_time;
        while idx < buckets.len() && buckets[idx].range.end <= insert_time {
            idx += 1;
        }
        if idx < buckets.len() && buckets[idx].range.begin <= insert_time {
            split[idx].push(block);
        }
    }
    split
}

/// Runs the `COUNT(*), SUM(nb_objects) ... GROUP BY date_bin(slice, insert_time)` query over
/// `insert_range`, under `identity_predicate` (the batch queries' own identity filter -- process +
/// stream-tag, or stream id) so the returned counts match what the batch queries themselves will
/// return -- the event-time predicate of the MIN/MAX pre-query would not do. Returns one
/// `(bucket_start, nb_blocks, nb_objects)` entry per non-empty bucket, ascending.
async fn fetch_bucket_counts(
    lakehouse: Arc<LakehouseContext>,
    blocks_view: &BlocksView,
    partitions: &PartitionCache,
    insert_range: TimeRange,
    slice: TimeDelta,
    identity_predicate: &str,
) -> Result<Vec<BucketCount>> {
    let slice_literal = interval_literal(slice);
    let begin_iso = insert_range.begin.to_rfc3339();
    let end_iso = insert_range.end.to_rfc3339();
    let sql = format!(
        r#"SELECT date_bin('{slice_literal}', insert_time) as bucket,
                  COUNT(*) as nb_blocks,
                  SUM(nb_objects) as nb_objects
             FROM source
             WHERE {identity_predicate}
             AND insert_time >= '{begin_iso}'
             AND insert_time < '{end_iso}'
             GROUP BY bucket
             ORDER BY bucket;"#
    );
    let filtered = partitions.filter_insert_range(insert_range).partitions;
    let df = instrument_named!(
        query_partitions(
            &lakehouse,
            blocks_view.get_file_schema(),
            Arc::new(filtered),
            &sql,
        ),
        "query_partitions"
    )
    .await?;
    let rbs = instrument_named!(df.collect(), "collect_bucket_counts").await?;
    let mut counts = vec![];
    for rb in &rbs {
        let bucket_column: &TimestampNanosecondArray = typed_column_by_name(rb, "bucket")?;
        let nb_blocks_column: &Int64Array = typed_column_by_name(rb, "nb_blocks")?;
        let nb_objects_column: &Int64Array = typed_column_by_name(rb, "nb_objects")?;
        for i in 0..rb.num_rows() {
            counts.push((
                DateTime::from_timestamp_nanos(bucket_column.value(i)),
                nb_blocks_column.value(i),
                nb_objects_column.value(i),
            ));
        }
    }
    Ok(counts)
}

/// The process-variant batch query: block columns plus `stream_id` only -- **no stream-level
/// column** (`streams.dependencies_metadata`/`objects_metadata`/`tags`/`properties`/`format`) may
/// be added to the `SELECT` list. Re-adding one would reintroduce the per-row stream-blob-copy
/// memory hazard the lean projection removes, with no test failing except the projection guard in
/// `analytics/tests/jit_batch_windows_tests.rs`. `array_has("streams.tags", ...)` stays in the
/// `WHERE` clause: filtering needs no projection.
pub fn process_batch_sql(process_id: &Uuid, stream_tag: &str, range: &TimeRange) -> String {
    let begin_iso = range.begin.to_rfc3339();
    let end_iso = range.end.to_rfc3339();
    format!(
        r#"SELECT block_id, stream_id, process_id, begin_time, end_time, begin_ticks, end_ticks, nb_objects, object_offset, payload_size, insert_time
             FROM source
             WHERE process_id = '{process_id}'
             AND array_has( "streams.tags", '{stream_tag}' )
             AND insert_time >= '{begin_iso}'
             AND insert_time < '{end_iso}'
             ORDER BY insert_time, block_id;"#
    )
}

/// Fetches and parses one batch window's worth of process-scoped blocks (`process_batch_sql`),
/// looking up each block's stream metadata in `stream_metadata` (built once per call to
/// `generate_process_jit_partitions` by `fetch_stream_metadata_map`) rather than rebuilding it per
/// row -- this is the lean projection's fetch-and-parse half.
///
/// A `stream_id` missing from `stream_metadata` is a hard error: the metadata pre-query and this
/// query share the same identity predicate and insert range, so it cannot happen unless something
/// is wrong.
pub async fn fetch_process_blocks(
    lakehouse: Arc<LakehouseContext>,
    blocks_view: &BlocksView,
    partitions: &PartitionCache,
    range: &TimeRange,
    process: Arc<ProcessMetadata>,
    stream_tag: &str,
    stream_metadata: &HashMap<Uuid, (Arc<StreamMetadata>, String)>,
) -> Result<Vec<Arc<PartitionSourceBlock>>> {
    let filtered = partitions.filter_insert_range(*range).partitions;
    let sql = process_batch_sql(&process.process_id, stream_tag, range);
    let df = instrument_named!(
        query_partitions(
            &lakehouse,
            blocks_view.get_file_schema(),
            Arc::new(filtered),
            &sql,
        ),
        "query_partitions"
    )
    .await?;
    let rbs = instrument_named!(df.collect(), "collect_partition_blocks").await?;
    let mut blocks = vec![];
    for rb in &rbs {
        let stream_id_column = string_column_by_name(rb, "stream_id")?;
        for ir in 0..rb.num_rows() {
            let block = block_from_batch_row(rb, ir).with_context(|| "block_from_batch_row")?;
            let stream_id = Uuid::parse_str(stream_id_column.value(ir)?)
                .with_context(|| "parsing stream_id")?;
            let (stream, format) = stream_metadata.get(&stream_id).with_context(|| {
                format!(
                    "fetch_process_blocks: missing stream metadata for stream {stream_id} \
                     (same predicate, same range as the metadata pre-query -- this should not \
                     happen)"
                )
            })?;
            blocks.push(Arc::new(PartitionSourceBlock {
                block,
                stream: stream.clone(),
                process: process.clone(),
                format: format.clone(),
            }));
        }
    }
    Ok(blocks)
}

/// Pre-query 3 (process variant only): fetches every stream's metadata once for the whole insert
/// range (stream metadata is immutable after registration), so the batched block queries can look
/// it up per block instead of rebuilding it. Mirrors `streams_view.rs`'s transform query, and
/// reads the result with the shared `stream_metadata_from_batch_row` helper -- `format` is kept
/// alongside in the map since it is not part of `StreamMetadata`.
async fn fetch_stream_metadata_map(
    lakehouse: Arc<LakehouseContext>,
    blocks_view: &BlocksView,
    partitions: &PartitionCache,
    insert_range: TimeRange,
    process: &ProcessMetadata,
    stream_tag: &str,
) -> Result<HashMap<Uuid, (Arc<StreamMetadata>, String)>> {
    let process_id = &process.process_id;
    let begin_iso = insert_range.begin.to_rfc3339();
    let end_iso = insert_range.end.to_rfc3339();
    let sql = format!(
        r#"SELECT stream_id,
               first_value("process_id")                    as process_id,
               first_value("streams.dependencies_metadata") as dependencies_metadata,
               first_value("streams.objects_metadata")      as objects_metadata,
               first_value("streams.tags")                  as tags,
               first_value("streams.properties")            as properties,
               first_value("streams.format")                as format
        FROM source
        WHERE process_id = '{process_id}'
        AND array_has( "streams.tags", '{stream_tag}' )
        AND insert_time >= '{begin_iso}'
        AND insert_time < '{end_iso}'
        GROUP BY stream_id;"#
    );
    let filtered = partitions.filter_insert_range(insert_range).partitions;
    let df = instrument_named!(
        query_partitions(
            &lakehouse,
            blocks_view.get_file_schema(),
            Arc::new(filtered),
            &sql,
        ),
        "query_partitions"
    )
    .await?;
    let rbs = instrument_named!(df.collect(), "collect_stream_metadata").await?;
    let mut map = HashMap::new();
    for rb in &rbs {
        let format_column = string_column_by_name(rb, "format")?;
        for ir in 0..rb.num_rows() {
            let stream = stream_metadata_from_batch_row(rb, ir)
                .with_context(|| "stream_metadata_from_batch_row")?;
            let format = format_column.value(ir)?.to_string();
            map.insert(stream.stream_id, (Arc::new(stream), format));
        }
    }
    Ok(map)
}

async fn get_insert_time_range(
    lakehouse: Arc<LakehouseContext>,
    blocks_view: &BlocksView,
    query_time_range: &TimeRange,
    stream: Arc<StreamMetadata>,
) -> Result<Option<TimeRange>> {
    // we would need a PartitionCache built from event time range and then filtered for insert time range
    let part_provider = LivePartitionProvider::new(lakehouse.lake().db_pool.clone());
    let partitions = part_provider
        .fetch(
            &blocks_view.get_view_set_name(),
            &blocks_view.get_view_instance_id(),
            Some(*query_time_range),
            blocks_view.get_file_schema_hash(),
        )
        .await?;
    let stream_id = &stream.stream_id;
    let begin_range_iso = query_time_range.begin.to_rfc3339();
    let end_range_iso = query_time_range.end.to_rfc3339();
    let sql = format!(
        r#"SELECT MIN(insert_time) as min_insert_time, MAX(insert_time) as max_insert_time
        FROM source
        WHERE stream_id = '{stream_id}'
        AND begin_time <= '{end_range_iso}'
        AND end_time >= '{begin_range_iso}';"#
    );
    let rbs = query_partitions(
        &lakehouse,
        blocks_view.get_file_schema(),
        Arc::new(partitions),
        &sql,
    )
    .await?
    .collect()
    .await?;
    if rbs.is_empty() {
        return Ok(None);
    }
    if rbs[0].num_rows() == 0 {
        return Ok(None);
    }
    let min_insert_time = get_single_row_primitive_value::<TimestampNanosecondType>(&rbs, 0)?;
    let max_insert_time = get_single_row_primitive_value::<TimestampNanosecondType>(&rbs, 1)?;
    Ok(Some(TimeRange::new(
        DateTime::from_timestamp_nanos(min_insert_time),
        DateTime::from_timestamp_nanos(max_insert_time),
    )))
}

/// Generates a segment of JIT partitions.
pub async fn generate_stream_jit_partitions_segment(
    config: &JitPartitionConfig,
    lakehouse: Arc<LakehouseContext>,
    blocks_view: &BlocksView,
    partitions: &PartitionCache,
    insert_time_range: &TimeRange,
    stream: Arc<StreamMetadata>,
    process: Arc<ProcessMetadata>,
) -> Result<Vec<SourceDataBlocksInMemory>> {
    let partitions = partitions
        .filter_insert_range(*insert_time_range)
        .partitions;

    let stream_id = &stream.stream_id;
    let begin_range_iso = insert_time_range.begin.to_rfc3339();
    let end_range_iso = insert_time_range.end.to_rfc3339();
    let sql = format!(
        r#"SELECT block_id, stream_id, process_id, begin_time, end_time, begin_ticks, end_ticks, nb_objects, object_offset, payload_size, insert_time, "streams.format"
             FROM source
             WHERE stream_id = '{stream_id}'
             AND insert_time >= '{begin_range_iso}'
             AND insert_time < '{end_range_iso}'
             ORDER BY insert_time, block_id;"#
    );

    let rbs = query_partitions(
        &lakehouse,
        blocks_view.get_file_schema(),
        Arc::new(partitions),
        &sql,
    )
    .await?
    .collect()
    .await?;

    let mut blocks = vec![];
    for rb in rbs {
        let format_column = string_column_by_name(&rb, "streams.format")?;
        for ir in 0..rb.num_rows() {
            let block = block_from_batch_row(&rb, ir).with_context(|| "block_from_batch_row")?;
            let format = format_column.value(ir)?.to_string();
            blocks.push(Arc::new(PartitionSourceBlock {
                block,
                stream: stream.clone(),
                process: process.clone(),
                format,
            }));
        }
    }

    Ok(group_blocks_into_partitions(config, blocks))
}

/// generate_stream_jit_partitions lists the partitiions that are needed to cover a time span
/// these partitions may not exist or they could be out of date
/// Generates JIT partitions for a given time range.
///
/// Requires `BucketSizing::Fixed`. Batches its block queries: `batch_windows`, derived from
/// per-bucket block counts, picks how many insert-time buckets one query covers, and each batch's
/// rows are then split back into per-bucket runs (`split_into_buckets`) and grouped independently
/// -- byte-identical to running `generate_stream_jit_partitions_segment` once per bucket, just
/// fewer, wider queries to get there. `generate_stream_jit_partitions_segment` itself is kept, not
/// called from here anymore.
#[span_fn]
pub async fn generate_stream_jit_partitions(
    config: &JitPartitionConfig,
    lakehouse: Arc<LakehouseContext>,
    blocks_view: &BlocksView,
    query_time_range: &TimeRange,
    stream: Arc<StreamMetadata>,
    process: Arc<ProcessMetadata>,
) -> Result<Vec<SourceDataBlocksInMemory>> {
    let BucketSizing::Fixed(slice) = config.bucket_sizing else {
        anyhow::bail!("generate_stream_jit_partitions requires BucketSizing::Fixed");
    };
    let insert_time_range = get_insert_time_range(
        lakehouse.clone(),
        blocks_view,
        query_time_range,
        stream.clone(),
    )
    .await?;
    if insert_time_range.is_none() {
        return Ok(vec![]);
    }
    let insert_time_range = insert_time_range.with_context(|| "missing insert_time_range")?;
    let insert_time_range = align_outward(insert_time_range, slice)?;
    let source_partitions = instrument_named!(
        PartitionCache::fetch_overlapping_insert_range_for_view(
            &lakehouse.lake().db_pool,
            blocks_view.get_view_set_name(),
            blocks_view.get_view_instance_id(),
            insert_time_range,
        ),
        "fetch_overlapping_insert_range_for_view"
    )
    .await?;

    let stream_id = stream.stream_id;
    let identity_predicate = format!("stream_id = '{stream_id}'");
    // A single-bucket insert_time_range can only ever produce one batch window, regardless of
    // the counts, so skip the COUNT(*) round trip entirely in that case.
    let bucket_counts = if insert_time_range.end - insert_time_range.begin <= slice {
        vec![]
    } else {
        fetch_bucket_counts(
            lakehouse.clone(),
            blocks_view,
            &source_partitions,
            insert_time_range,
            slice,
            &identity_predicate,
        )
        .await?
    };
    let planned = config.plan(insert_time_range, slice, slice, &bucket_counts);

    let windows: Vec<TimeRange> = batch_windows(&planned, config.target_rows_per_query).collect();
    debug!(
        "generate_stream_jit_partitions: stream={stream_id}: derived {} batch window(s) from {} \
         non-empty bucket(s) (~{} row(s) total, target_rows_per_query={})",
        windows.len(),
        planned.len(),
        planned.iter().map(|b| b.nb_blocks).sum::<i64>(),
        config.target_rows_per_query
    );

    let mut partitions = vec![];
    for batch_range in windows {
        let filtered = source_partitions
            .filter_insert_range(batch_range)
            .partitions;
        let begin_iso = batch_range.begin.to_rfc3339();
        let end_iso = batch_range.end.to_rfc3339();
        let sql = format!(
            r#"SELECT block_id, stream_id, process_id, begin_time, end_time, begin_ticks, end_ticks, nb_objects, object_offset, payload_size, insert_time, "streams.format"
                 FROM source
                 WHERE stream_id = '{stream_id}'
                 AND insert_time >= '{begin_iso}'
                 AND insert_time < '{end_iso}'
                 ORDER BY insert_time, block_id;"#
        );
        let df = instrument_named!(
            query_partitions(
                &lakehouse,
                blocks_view.get_file_schema(),
                Arc::new(filtered),
                &sql,
            ),
            "query_partitions"
        )
        .await?;
        let rbs = instrument_named!(df.collect(), "collect_partition_blocks").await?;

        let mut blocks = vec![];
        for rb in rbs {
            let format_column = string_column_by_name(&rb, "streams.format")?;
            for ir in 0..rb.num_rows() {
                let block =
                    block_from_batch_row(&rb, ir).with_context(|| "block_from_batch_row")?;
                let format = format_column.value(ir)?.to_string();
                blocks.push(Arc::new(PartitionSourceBlock {
                    block,
                    stream: stream.clone(),
                    process: process.clone(),
                    format,
                }));
            }
        }
        let window_buckets: Vec<PlannedBucket> = planned
            .iter()
            .filter(|b| b.range.begin >= batch_range.begin && b.range.end <= batch_range.end)
            .copied()
            .collect();
        for bucket_blocks in split_into_buckets(blocks, &window_buckets) {
            partitions.append(&mut group_blocks_into_partitions(config, bucket_blocks));
        }
    }
    Ok(partitions)
}

/// generate_process_jit_partitions lists the partitions that are needed to cover a time span for a specific process
/// these partitions may not exist or they could be out of date
/// Generates JIT partitions for a given time range filtered by process, grouped by the insert-time
/// bucket `plan_buckets` assigned them to.
///
/// The bucket width comes from `JitPartitionConfig::bucket_sizing`: a fixed slice, or the rung
/// `select_rung(query_time_range)` picks. Batches its block queries the same way
/// `generate_stream_jit_partitions` does, and additionally applies the lean projection:
/// `fetch_stream_metadata_map` fetches every stream's metadata once for the whole insert range,
/// and each batch's `fetch_process_blocks` call looks it up per block instead of projecting stream
/// blobs onto every row and rebuilding `StreamMetadata` per row. The grouping call is inlined
/// into the batch loop below.
#[span_fn]
pub async fn generate_process_jit_partitions(
    config: &JitPartitionConfig,
    lakehouse: Arc<LakehouseContext>,
    blocks_view: &BlocksView,
    query_time_range: &TimeRange,
    process: Arc<ProcessMetadata>,
    stream_tag: &str,
) -> Result<Vec<JitBucket>> {
    let (rung, finest) = config.rungs(query_time_range);
    // Get insert time range for all blocks in this process
    let part_provider = LivePartitionProvider::new(lakehouse.lake().db_pool.clone());
    let view_set_name = blocks_view.get_view_set_name();
    let view_instance_id = blocks_view.get_view_instance_id();
    let partitions = instrument_named!(
        part_provider.fetch(
            &view_set_name,
            &view_instance_id,
            Some(*query_time_range),
            blocks_view.get_file_schema_hash(),
        ),
        "live_partition_provider_fetch"
    )
    .await?;

    let process_id = &process.process_id;
    let begin_range_iso = query_time_range.begin.to_rfc3339();
    let end_range_iso = query_time_range.end.to_rfc3339();
    let sql = format!(
        r#"SELECT MIN(insert_time) as min_insert_time, MAX(insert_time) as max_insert_time
        FROM source
        WHERE process_id = '{process_id}'
        AND array_has( "streams.tags", '{stream_tag}' )
        AND begin_time <= '{end_range_iso}'
        AND end_time >= '{begin_range_iso}';"#
    );

    let df = instrument_named!(
        query_partitions(
            &lakehouse,
            blocks_view.get_file_schema(),
            Arc::new(partitions),
            &sql,
        ),
        "query_partitions"
    )
    .await?;
    let rbs = instrument_named!(df.collect(), "collect_insert_time_range").await?;

    if rbs.is_empty() || rbs[0].num_rows() == 0 {
        return Ok(vec![]);
    }

    let min_insert_time = get_single_row_primitive_value::<TimestampNanosecondType>(&rbs, 0)?;
    let max_insert_time = get_single_row_primitive_value::<TimestampNanosecondType>(&rbs, 1)?;

    if min_insert_time == 0 || max_insert_time == 0 {
        return Ok(vec![]);
    }

    let insert_time_range = align_outward(
        TimeRange::new(
            DateTime::from_timestamp_nanos(min_insert_time),
            DateTime::from_timestamp_nanos(max_insert_time),
        ),
        rung,
    )?;

    let source_partitions = instrument_named!(
        PartitionCache::fetch_overlapping_insert_range_for_view(
            &lakehouse.lake().db_pool,
            blocks_view.get_view_set_name(),
            blocks_view.get_view_instance_id(),
            insert_time_range,
        ),
        "fetch_overlapping_insert_range_for_view"
    )
    .await?;

    let identity_predicate =
        format!(r#"process_id = '{process_id}' AND array_has( "streams.tags", '{stream_tag}' )"#);
    // A single-bucket insert_time_range can only ever produce one batch window, regardless of
    // the counts, so skip the COUNT(*) round trip entirely in that case.
    let bucket_counts = if insert_time_range.end - insert_time_range.begin <= finest {
        vec![]
    } else {
        fetch_bucket_counts(
            lakehouse.clone(),
            blocks_view,
            &source_partitions,
            insert_time_range,
            finest,
            &identity_predicate,
        )
        .await?
    };
    let planned = config.plan(insert_time_range, rung, finest, &bucket_counts);

    let stream_metadata = fetch_stream_metadata_map(
        lakehouse.clone(),
        blocks_view,
        &source_partitions,
        insert_time_range,
        &process,
        stream_tag,
    )
    .await?;

    let windows: Vec<TimeRange> = batch_windows(&planned, config.target_rows_per_query).collect();
    debug!(
        "generate_process_jit_partitions: process={process_id} stream_tag={stream_tag}: derived \
         {} batch window(s) from {} planned bucket(s) (~{} row(s) total, \
         target_rows_per_query={})",
        windows.len(),
        planned.len(),
        planned.iter().map(|b| b.nb_blocks).sum::<i64>(),
        config.target_rows_per_query
    );

    let mut buckets = vec![];
    for batch_range in windows {
        let blocks = fetch_process_blocks(
            lakehouse.clone(),
            blocks_view,
            &source_partitions,
            &batch_range,
            process.clone(),
            stream_tag,
            &stream_metadata,
        )
        .await?;
        let window_buckets: Vec<PlannedBucket> = planned
            .iter()
            .filter(|b| b.range.begin >= batch_range.begin && b.range.end <= batch_range.end)
            .copied()
            .collect();
        for (planned, bucket_blocks) in window_buckets
            .iter()
            .zip(split_into_buckets(blocks, &window_buckets))
        {
            let specs = group_blocks_into_partitions(config, bucket_blocks);
            if !specs.is_empty() {
                buckets.push(JitBucket {
                    planned: *planned,
                    specs,
                });
            }
        }
    }
    Ok(buckets)
}

/// One `lakehouse_partitions` candidate row, as fetched by `fetch_freshness_candidates`:
/// `(begin_insert_time, end_insert_time, file_schema_hash, source_data_hash)`. Fields are `pub` so
/// `analytics/tests/` can build rows directly, without a live database.
#[derive(Debug, Clone)]
pub struct PartitionFreshnessRow {
    pub begin_insert_time: DateTime<Utc>,
    pub end_insert_time: DateTime<Utc>,
    pub file_schema_hash: Vec<u8>,
    pub source_data_hash: Vec<u8>,
}

/// Fetches every `lakehouse_partitions` row for `view_meta` whose insert range *inclusively
/// overlaps* `range` (`begin_insert_time <= range.end AND end_insert_time >= range.begin`), with
/// the narrowing applied in Rust by the caller (`spec_is_up_to_date`, `resolve_bucket_coverage`).
pub async fn fetch_freshness_candidates(
    pool: &sqlx::PgPool,
    view_meta: &ViewMetadata,
    range: TimeRange,
) -> Result<Vec<PartitionFreshnessRow>> {
    let rows = sqlx::query(
        "SELECT begin_insert_time, end_insert_time, file_schema_hash, source_data_hash
         FROM lakehouse_partitions
         WHERE view_set_name = $1
         AND view_instance_id = $2
         AND begin_insert_time <= $3
         AND end_insert_time >= $4
         ;",
    )
    .bind(&*view_meta.view_set_name)
    .bind(&*view_meta.view_instance_id)
    .bind(range.end)
    .bind(range.begin)
    .fetch_all(pool)
    .await
    .with_context(|| "fetching freshness candidates")?;
    rows.into_iter()
        .map(|r| {
            Ok(PartitionFreshnessRow {
                begin_insert_time: r.try_get("begin_insert_time")?,
                end_insert_time: r.try_get("end_insert_time")?,
                file_schema_hash: r.try_get("file_schema_hash")?,
                source_data_hash: r.try_get("source_data_hash")?,
            })
        })
        .collect()
}

/// Filters `candidates` down to the rows with exactly the spec's insert range, then applies the
/// `rows.len() == 1` / `file_schema_hash` / exact object-count checks. Used by the
/// `BlockOrder::EventTime` views (`thread_spans`/`net_spans`) only: their cut points can move
/// between `jit_update` runs, so a later run's spec can be narrower than an already-written
/// partition that still overlaps it, and an overlap test would call that stale, wider partition up
/// to date and leave it in place forever. Exact equality reports "not up to date" instead, so the
/// write falls through to `RetireMatch::Overlap` (see `write_partition.rs`). The `InsertTime` views
/// judge freshness per bucket with `resolve_bucket_coverage`.
pub fn spec_is_up_to_date(
    view_meta: &ViewMetadata,
    spec: &SourceDataBlocksInMemory,
    candidates: &[PartitionFreshnessRow],
) -> Result<bool> {
    let (min_insert_time, max_insert_time) =
        get_part_insert_time_range(spec).with_context(|| "get_event_time_range")?;
    let desc = format!(
        "[{}, {}] {} {}",
        min_insert_time.to_rfc3339(),
        max_insert_time.to_rfc3339(),
        *view_meta.view_set_name,
        *view_meta.view_instance_id,
    );

    let matches: Vec<&PartitionFreshnessRow> = candidates
        .iter()
        .filter(|r| r.begin_insert_time == min_insert_time && r.end_insert_time == max_insert_time)
        .collect();
    if matches.len() != 1 {
        debug!("{desc}: found {} partitions (expected 1)", matches.len());
        for (i, r) in matches.iter().enumerate() {
            let source_row_count = hash_to_object_count(&r.source_data_hash)?;
            debug!(
                "{desc}: partition {}: file_schema_hash={:?}, source_rows={}",
                i, r.file_schema_hash, source_row_count
            );
        }
        info!("{desc}: found {} partitions", matches.len());
        return Ok(false);
    }
    let r = matches[0];
    if r.file_schema_hash != view_meta.file_schema_hash {
        // this is dangerous because we could be creating a new partition smaller than the old one, which is not supported.
        // let's make sure there is no old data loitering
        warn!("{desc}: found matching partition with different file schema");
        return Ok(false);
    }
    let existing_count = hash_to_object_count(&r.source_data_hash)?;
    let required_count = hash_to_object_count(&spec.block_ids_hash)?;
    if existing_count != required_count {
        info!(
            "{desc}: existing partition object count differs \
             (existing={existing_count}, required={required_count}): creating a new partition"
        );
        return Ok(false);
    }
    info!("{desc}: partition up to date");
    Ok(true)
}

/// is_jit_partition_up_to_date compares a partition spec with the partitions that exist to know if it should be recreated
/// Checks if a JIT partition is up to date.
///
/// Fetches the spec's own candidate range (`fetch_freshness_candidates`) and runs the shared
/// matcher (`spec_is_up_to_date`) against it. Used by the `BlockOrder::EventTime` callers
/// (`net_spans`/`thread_spans`), whose checks stay per-spec, interleaved with retire logic inside
/// their own `update_partition`.
#[span_fn]
pub async fn is_jit_partition_up_to_date(
    pool: &sqlx::PgPool,
    view_meta: ViewMetadata,
    spec: &SourceDataBlocksInMemory,
) -> Result<bool> {
    let (min_insert_time, max_insert_time) =
        get_part_insert_time_range(spec).with_context(|| "get_event_time_range")?;
    let candidates = instrument_named!(
        fetch_freshness_candidates(
            pool,
            &view_meta,
            TimeRange::new(min_insert_time, max_insert_time),
        ),
        "sql_select_matching_partitions"
    )
    .await?;
    spec_is_up_to_date(&view_meta, spec, &candidates)
}

/// What a bucket needs: nothing, or its specs written with the given retire mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    Covered,
    Stale { retire_match: RetireMatch },
}

/// What to do after a mandatory write lost an overlap race.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictRetry {
    /// A concurrent writer already covers the bucket.
    Drop,
    /// Write again, retiring with this mode.
    Retry { retire_match: RetireMatch },
}

/// A bucket's specs boiled down to what coverage needs.
struct BucketFacts {
    range: TimeRange,
    spec_hulls: Vec<TimeRange>,
    hull: TimeRange,
    required: i64,
}

fn bucket_facts(bucket: &JitBucket) -> Result<BucketFacts> {
    let mut spec_hulls = vec![];
    let mut required = 0;
    for spec in &bucket.specs {
        spec_hulls
            .push(blocks_insert_time_range(&spec.blocks).with_context(|| "empty partition spec")?);
        required += hash_to_object_count(&spec.block_ids_hash)?;
    }
    let begin = spec_hulls
        .iter()
        .map(|h| h.begin)
        .min()
        .with_context(|| "bucket without specs")?;
    let end = spec_hulls
        .iter()
        .map(|h| h.end)
        .max()
        .with_context(|| "bucket without specs")?;
    Ok(BucketFacts {
        range: bucket.planned.range,
        spec_hulls,
        hull: TimeRange::new(begin, end),
        required,
    })
}

/// A row's inclusive `[begin, end]` insert range against a half-open bucket.
fn row_intersects(row: &PartitionFreshnessRow, bucket: TimeRange) -> bool {
    row.begin_insert_time < bucket.end && row.end_insert_time >= bucket.begin
}

pub(crate) fn row_inside(row: &PartitionFreshnessRow, bucket: TimeRange) -> bool {
    row.begin_insert_time >= bucket.begin && row.end_insert_time < bucket.end
}

/// Judges one bucket against `rows` (the rows that count as present). The retire mode of a stale
/// verdict is derived from `raw_rows`: a row that the bucket's own write will not retire through
/// `Containment` must be retired through `Overlap`, or the insert trips the no-overlap constraint,
/// even when `rows` no longer lists it because another write is about to retire it.
fn judge_bucket(
    facts: &BucketFacts,
    view_meta: &ViewMetadata,
    rows: &[PartitionFreshnessRow],
    raw_rows: &[PartitionFreshnessRow],
) -> Result<Coverage> {
    let (inner, outer): (Vec<&PartitionFreshnessRow>, Vec<&PartitionFreshnessRow>) = rows
        .iter()
        .filter(|r| row_intersects(r, facts.range))
        .partition(|r| row_inside(r, facts.range));
    let schema_ok = |r: &&PartitionFreshnessRow| r.file_schema_hash == view_meta.file_schema_hash;
    let covered = if outer.is_empty() {
        // Fine-or-equal rungs: the bucket's own rows must cover every required object. A surplus
        // is harmless: retention deletes blocks before it retires the rows that hold them, so a
        // surviving row can count more objects than the blocks that remain. Rewriting would
        // trip the no-overlap constraint, because the retire misses the row (it lies outside the
        // remaining specs' hull).
        let mut inner_count = 0;
        for r in &inner {
            inner_count += hash_to_object_count(&r.source_data_hash)?;
        }
        !inner.is_empty() && inner_count >= facts.required && inner.iter().all(schema_ok)
    } else if inner.is_empty() && outer.len() == 1 {
        // Coarser rung: one row written while its bucket was closed, so it holds everything.
        let r = outer[0];
        r.begin_insert_time <= facts.hull.begin
            && r.end_insert_time >= facts.hull.end
            && schema_ok(&r)
            && hash_to_object_count(&r.source_data_hash)? >= facts.required
    } else {
        false
    };
    if covered {
        return Ok(Coverage::Covered);
    }
    let raw_has_outer = raw_rows
        .iter()
        .any(|r| row_intersects(r, facts.range) && !row_inside(r, facts.range));
    Ok(Coverage::Stale {
        retire_match: if raw_has_outer {
            RetireMatch::Overlap
        } else {
            RetireMatch::Containment
        },
    })
}

/// Judges every bucket against the candidate rows and cascades: writing a stale bucket retires
/// rows (those inside its spec hulls for `Containment`, those intersecting them for `Overlap`),
/// which can strip coverage from a bucket that relied on one of them. Verdicts only move from
/// covered to stale, so the loop ends in at most `buckets.len()` passes, and the query that
/// triggers the writes never reads a bucket its own writes just emptied.
///
/// `buckets` are ascending and non-overlapping; returns verdicts parallel to them. Does no I/O.
pub fn resolve_bucket_coverage(
    buckets: &[JitBucket],
    view_meta: &ViewMetadata,
    candidates: &[PartitionFreshnessRow],
) -> Result<Vec<Coverage>> {
    let facts: Vec<BucketFacts> = buckets.iter().map(bucket_facts).collect::<Result<_>>()?;
    let mut verdicts: Vec<Coverage> = facts
        .iter()
        .map(|f| judge_bucket(f, view_meta, candidates, candidates))
        .collect::<Result<_>>()?;
    loop {
        let mut doomed = vec![false; candidates.len()];
        for (f, verdict) in facts.iter().zip(&verdicts) {
            let Coverage::Stale { retire_match } = verdict else {
                continue;
            };
            for hull in &f.spec_hulls {
                for (row, doomed) in candidates.iter().zip(doomed.iter_mut()) {
                    *doomed |= match retire_match {
                        RetireMatch::Containment => {
                            row.begin_insert_time >= hull.begin && row.end_insert_time <= hull.end
                        }
                        RetireMatch::Overlap => {
                            row.begin_insert_time <= hull.end && row.end_insert_time >= hull.begin
                        }
                    };
                }
            }
        }
        let remaining: Vec<PartitionFreshnessRow> = candidates
            .iter()
            .zip(&doomed)
            .filter(|(_, doomed)| !**doomed)
            .map(|(row, _)| row.clone())
            .collect();
        let mut changed = false;
        for (f, verdict) in facts.iter().zip(verdicts.iter_mut()) {
            if *verdict == Coverage::Covered {
                let rejudged = judge_bucket(f, view_meta, &remaining, candidates)?;
                if rejudged != Coverage::Covered {
                    *verdict = rejudged;
                    changed = true;
                }
            }
        }
        if !changed {
            return Ok(verdicts);
        }
    }
}

/// Decides what to do with a bucket whose write hit the no-overlap constraint, from the rows
/// fetched again afterwards. A concurrent writer that committed after the first retire SELECT is
/// listed in `refetched`, so the retry's retire mode is derived from it.
pub fn decide_conflict_retry(
    bucket: &JitBucket,
    view_meta: &ViewMetadata,
    refetched: &[PartitionFreshnessRow],
) -> Result<ConflictRetry> {
    let facts = bucket_facts(bucket)?;
    Ok(
        match judge_bucket(&facts, view_meta, refetched, refetched)? {
            Coverage::Covered => ConflictRetry::Drop,
            Coverage::Stale { retire_match } => ConflictRetry::Retry { retire_match },
        },
    )
}

#[allow(clippy::too_many_arguments)]
async fn write_spec_with_conflict_retry(
    lakehouse: &LakehouseContext,
    view: &dyn View,
    view_meta: &ViewMetadata,
    bucket: &JitBucket,
    spec: &SourceDataBlocksInMemory,
    retire_match: RetireMatch,
    same_run_ranges: &[TimeRange],
    block_processors: &Arc<BlockProcessorMap>,
) -> Result<()> {
    let attempt = |retire_match: RetireMatch| {
        write_partition_from_blocks(
            lakehouse.lake().clone(),
            view_meta.clone(),
            view.get_file_schema(),
            spec.clone(),
            block_processors.clone(),
            retire_match,
            same_run_ranges.to_vec(),
        )
    };
    let err = match attempt(retire_match).await {
        Ok(()) => return Ok(()),
        Err(err) => err,
    };
    if !err
        .chain()
        .any(|e| e.downcast_ref::<PartitionOverlapConflict>().is_some())
    {
        return Err(err);
    }
    let facts = bucket_facts(bucket)?;
    let refetched = fetch_freshness_candidates(
        &lakehouse.lake().db_pool,
        view_meta,
        TimeRange::new(
            facts.range.begin.min(facts.hull.begin),
            facts.range.end.max(facts.hull.end),
        ),
    )
    .await?;
    match decide_conflict_retry(bucket, view_meta, &refetched)? {
        ConflictRetry::Drop => {
            info!(
                "bucket [{}, {}) is now covered by a concurrent writer: dropping write",
                facts.range.begin.to_rfc3339(),
                facts.range.end.to_rfc3339()
            );
            Ok(())
        }
        ConflictRetry::Retry { retire_match } => attempt(retire_match).await,
    }
}

/// Brings an `InsertTime` JIT view of one process up to date for `query_range`: sizes buckets from
/// the range, writes the stale ones, then promotes closed history to the range's rung.
///
/// `view` is owned because promotion hands it to `create_merged_partition`.
#[span_fn]
pub async fn update_insert_time_jit_view(
    lakehouse: Arc<LakehouseContext>,
    view: Arc<dyn View>,
    process: Arc<ProcessMetadata>,
    query_range: TimeRange,
    stream_tag: &str,
    block_processors: Arc<BlockProcessorMap>,
) -> Result<()> {
    let config = JitPartitionConfig::default();
    let query_rung = select_rung(query_range);
    let blocks_view = BlocksView::new(lakehouse.default_audience())?;
    let buckets = generate_process_jit_partitions(
        &config,
        lakehouse.clone(),
        &blocks_view,
        &query_range,
        process,
        stream_tag,
    )
    .await
    .with_context(|| "generate_process_jit_partitions")?;
    let (Some(first), Some(last)) = (buckets.first(), buckets.last()) else {
        return Ok(());
    };
    let view_meta = view.get_meta();
    let candidates = instrument_named!(
        fetch_freshness_candidates(
            &lakehouse.lake().db_pool,
            &view_meta,
            TimeRange::new(first.planned.range.begin, last.planned.range.end),
        ),
        "sql_select_freshness_candidates"
    )
    .await?;
    let verdicts = resolve_bucket_coverage(&buckets, &view_meta, &candidates)
        .with_context(|| "resolve_bucket_coverage")?;

    // Insert ranges this run already handled, so a later `Overlap` retire never retires a row of
    // this same run -- see `RetireMatch::Overlap`'s docs.
    let mut same_run_ranges: Vec<TimeRange> = vec![];
    for (bucket, verdict) in buckets.iter().zip(&verdicts) {
        for spec in &bucket.specs {
            if let Coverage::Stale { retire_match } = verdict {
                write_spec_with_conflict_retry(
                    &lakehouse,
                    view.as_ref(),
                    &view_meta,
                    bucket,
                    spec,
                    *retire_match,
                    &same_run_ranges,
                    &block_processors,
                )
                .await
                .with_context(|| "write_partition_from_blocks")?;
            }
            same_run_ranges.push(blocks_insert_time_range(&spec.blocks)?);
        }
    }

    let targets = promotion_candidates(
        &buckets,
        &verdicts,
        &candidates,
        query_rung,
        config.max_nb_objects,
    );
    promote_closed_buckets(lakehouse, view, targets, config.max_nb_objects).await;
    Ok(())
}

/// get_event_time_range returns the time range covered by a partition spec
/// Returns the insert time range covered by a partition spec.
fn get_part_insert_time_range(
    spec: &SourceDataBlocksInMemory,
) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let range = blocks_insert_time_range(&spec.blocks)
        .with_context(|| "empty partition should not exist")?;
    Ok((range.begin, range.end))
}

/// Writes a partition from a set of blocks, retiring the rows `retire_match` selects (see
/// `RetireMatch`; `same_run_ranges` is only meaningful for `Overlap`).
///
/// `block_processors` keys per-block dispatch on the source block's `format`;
/// see `BlockPartitionSpec::write` for the unknown-format behavior.
#[span_fn]
pub async fn write_partition_from_blocks(
    lake: Arc<DataLakeConnection>,
    view_metadata: ViewMetadata,
    schema: Arc<Schema>,
    source_data: SourceDataBlocksInMemory,
    block_processors: Arc<BlockProcessorMap>,
    retire_match: RetireMatch,
    same_run_ranges: Vec<TimeRange>,
) -> Result<()> {
    if source_data.blocks.is_empty() {
        anyhow::bail!("empty partition spec");
    }
    let insert_range = blocks_insert_time_range(&source_data.blocks)
        .with_context(|| "blocks_insert_time_range")?;
    let block_spec = BlockPartitionSpec {
        view_metadata,
        schema,
        insert_range,
        source_data: Arc::new(source_data),
        block_processors,
        retire_match,
        same_run_ranges,
    };
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    block_spec
        .write(lake, null_response_writer)
        .await
        .with_context(|| "block_spec.write")?;
    Ok(())
}
