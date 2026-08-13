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
//! `begin_ticks`/`end_ticks`. Whether those can overlap slightly at a cut point depends on the
//! producer: `micromegas_tracing`-produced streams stamp the replacement block's `begin` before
//! closing the outgoing block, so consecutive blocks overlap; the Unreal producer stamps a single
//! timestamp for both, so they touch exactly. See the ordering-invariant notes on
//! `View::get_scan_output_ordering`.

use super::{
    block_partition_spec::{BlockPartitionSpec, BlockProcessorMap},
    blocks_view::BlocksView,
    lakehouse_context::LakehouseContext,
    partition_cache::{LivePartitionProvider, QueryPartitionProvider},
    partition_source_data::{PartitionSourceBlock, SourceDataBlocksInMemory},
    view::{View, ViewMetadata},
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
use chrono::DurationRound;
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

/// Configuration for Just-In-Time (JIT) partition generation.
pub struct JitPartitionConfig {
    pub max_nb_objects: i64,
    pub max_insert_time_slice: TimeDelta,
    pub block_order: BlockOrder,
    /// Soft row-count target for a single batched block query (see `batch_windows`). Consecutive
    /// insert-time buckets are packed into one query up to this many blocks, derived from a
    /// per-bucket `COUNT(*)` run before the batch queries themselves -- see the module's Adaptive
    /// batch width design notes. A config field rather than a bare constant so DB-gated tests can
    /// lower it to force a run to split into more than one batch.
    pub target_rows_per_query: i64,
}

impl Default for JitPartitionConfig {
    fn default() -> Self {
        JitPartitionConfig {
            max_nb_objects: 20 * 1024 * 1024,
            max_insert_time_slice: TimeDelta::hours(1),
            block_order: BlockOrder::InsertTime,
            target_rows_per_query: 250_000,
        }
    }
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
/// open. That distinction matters because it is producer-dependent:
/// `micromegas_tracing::dispatch`'s flush paths stamp the replacement block's `begin` before closing
/// the outgoing block, so consecutive blocks always overlap by the cost of the buffer swap, whereas
/// the Unreal producer stamps a single timestamp for both sides and its blocks touch exactly. An
/// equality test would break the chain on every seam for the former.
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
///   `BlockOrder::EventTime` arm of `is_jit_partition_up_to_date` would see more than one row for
///   that range and never report the partitions up to date.
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
/// grows past the soft limit and a `debug!` logs it. See
/// `tasks/1429_jit_event_time_block_ordering_plan.md` §3 for the full derivation.
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

/// Packs consecutive insert-time buckets of `[insert_range.begin, insert_range.end)` (stepping by
/// `slice`, so every bucket edge is `slice`-aligned to `insert_range.begin`) into batch query
/// windows, greedily closing a window just before it would exceed `target_rows_per_query`.
///
/// `bucket_counts` holds one `(bucket_start, nb_blocks)` pair per *non-empty* bucket, ascending;
/// any bucket missing from it is treated as holding zero blocks (and so never forces a close on
/// its own). See the module's Adaptive batch width design notes for the derivation and the
/// single-oversized-bucket residual case (a bucket whose own count already exceeds
/// `target_rows_per_query` still forms one batch on its own -- the loop never splits a bucket).
///
/// Batch edges are always bucket-aligned and batches tile `insert_range` with no gaps or overlaps,
/// so which width is picked cannot change the specs `group_blocks_into_partitions` later emits
/// per bucket -- only how many SQL queries it takes to gather them.
pub fn batch_windows(
    insert_range: TimeRange,
    slice: TimeDelta,
    bucket_counts: &[(DateTime<Utc>, i64)],
    target_rows_per_query: i64,
) -> impl Iterator<Item = TimeRange> {
    let mut windows = Vec::new();
    let mut running: i64 = 0;
    let mut batch_begin = insert_range.begin;
    let mut bucket_begin = insert_range.begin;
    let mut counts_idx = 0usize;
    while bucket_begin < insert_range.end {
        // bucket_counts is ascending and every bucket we walk here is >= the previous one, so a
        // single forward-moving index suffices (no need to search backwards).
        while counts_idx < bucket_counts.len() && bucket_counts[counts_idx].0 < bucket_begin {
            counts_idx += 1;
        }
        let nb_blocks =
            if counts_idx < bucket_counts.len() && bucket_counts[counts_idx].0 == bucket_begin {
                let n = bucket_counts[counts_idx].1;
                counts_idx += 1;
                n
            } else {
                0
            };
        if running > 0 && running + nb_blocks > target_rows_per_query {
            windows.push(TimeRange::new(batch_begin, bucket_begin));
            running = 0;
            batch_begin = bucket_begin;
        }
        running += nb_blocks;
        bucket_begin += slice;
    }
    windows.push(TimeRange::new(batch_begin, insert_range.end));
    windows.into_iter()
}

/// Renders a `TimeDelta` as arrow-parsable interval text (e.g. `"3600 seconds"`) for use with
/// DataFusion's `date_bin`. `TimeDelta`'s own `Display` yields ISO-8601 (`PT3600S`), which
/// `date_bin` cannot parse -- see the module's Adaptive batch width design notes.
fn interval_literal(slice: TimeDelta) -> String {
    format!("{} seconds", slice.num_seconds())
}

/// Splits an insert-time-sorted block list into consecutive runs sharing the same
/// `insert_time.duration_trunc(slice)` bucket. The SQL feeding this is `ORDER BY insert_time,
/// block_id`, so buckets are contiguous runs in the list -- no sorting or grouping by key needed,
/// just a scan that closes a run whenever the bucket changes.
fn split_into_buckets(
    blocks: Vec<Arc<PartitionSourceBlock>>,
    slice: TimeDelta,
) -> Result<Vec<Vec<Arc<PartitionSourceBlock>>>> {
    let mut buckets = vec![];
    let mut current: Vec<Arc<PartitionSourceBlock>> = vec![];
    let mut current_bucket: Option<DateTime<Utc>> = None;
    for block in blocks {
        let bucket = block.block.insert_time.duration_trunc(slice)?;
        if current_bucket != Some(bucket) {
            if !current.is_empty() {
                buckets.push(std::mem::take(&mut current));
            }
            current_bucket = Some(bucket);
        }
        current.push(block);
    }
    if !current.is_empty() {
        buckets.push(current);
    }
    Ok(buckets)
}

/// Runs the per-bucket `COUNT(*) ... GROUP BY date_bin(slice, insert_time)` query over
/// `insert_range`, under `identity_predicate` (the batch queries' own identity filter -- process +
/// stream-tag, or stream id) so the returned counts match what the batch queries themselves will
/// return; see the module's Adaptive batch width design notes for why the event-time predicate of
/// the MIN/MAX pre-query would not do. Returns one `(bucket_start, nb_blocks)` pair per non-empty
/// bucket, ascending.
async fn fetch_bucket_counts(
    lakehouse: Arc<LakehouseContext>,
    blocks_view: &BlocksView,
    partitions: &PartitionCache,
    insert_range: TimeRange,
    slice: TimeDelta,
    identity_predicate: &str,
) -> Result<Vec<(DateTime<Utc>, i64)>> {
    let slice_literal = interval_literal(slice);
    let begin_iso = insert_range.begin.to_rfc3339();
    let end_iso = insert_range.end.to_rfc3339();
    let sql = format!(
        r#"SELECT date_bin('{slice_literal}', insert_time) as bucket, COUNT(*) as nb_blocks
             FROM source
             WHERE {identity_predicate}
             AND insert_time >= '{begin_iso}'
             AND insert_time < '{end_iso}'
             GROUP BY bucket
             ORDER BY bucket;"#
    );
    let filtered = partitions.filter_insert_range(insert_range).partitions;
    let reader_factory = lakehouse.reader_factory().clone();
    let df = instrument_named!(
        query_partitions(
            lakehouse.runtime().clone(),
            reader_factory,
            lakehouse.lake().blob_storage.inner(),
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
        for i in 0..rb.num_rows() {
            counts.push((
                DateTime::from_timestamp_nanos(bucket_column.value(i)),
                nb_blocks_column.value(i),
            ));
        }
    }
    Ok(counts)
}

/// The process-variant batch query: block columns plus `stream_id` only -- **no stream-level
/// column** (`streams.dependencies_metadata`/`objects_metadata`/`tags`/`properties`/`format`) may
/// be added to the `SELECT` list. Re-adding one would reintroduce the per-row stream-blob-copy
/// memory hazard the lean projection removes, with no test failing except the projection guard in
/// `analytics/tests/jit_batch_windows_tests.rs` -- see the module's "Why the lean projection is in
/// scope" design notes. `array_has("streams.tags", ...)` stays in the `WHERE` clause: filtering
/// needs no projection.
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
/// row -- this is the lean projection's fetch-and-parse half; see the module's "Why the lean
/// projection is in scope" design notes.
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
    let reader_factory = lakehouse.reader_factory().clone();
    let df = instrument_named!(
        query_partitions(
            lakehouse.runtime().clone(),
            reader_factory,
            lakehouse.lake().blob_storage.inner(),
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
    let reader_factory = lakehouse.reader_factory().clone();
    let df = instrument_named!(
        query_partitions(
            lakehouse.runtime().clone(),
            reader_factory,
            lakehouse.lake().blob_storage.inner(),
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
    let reader_factory = lakehouse.reader_factory().clone();
    let rbs = query_partitions(
        lakehouse.runtime().clone(),
        reader_factory,
        lakehouse.lake().blob_storage.inner(),
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

    let reader_factory = lakehouse.reader_factory().clone();
    let rbs = query_partitions(
        lakehouse.runtime().clone(),
        reader_factory,
        lakehouse.lake().blob_storage.inner(),
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
/// Batches its block queries (see the module's Design notes): `batch_windows`, derived from a
/// per-bucket `COUNT(*)`, picks how many insert-time buckets one query covers, and each batch's
/// rows are then split back into per-bucket runs (`split_into_buckets`) and grouped independently
/// -- byte-identical to running `generate_stream_jit_partitions_segment` once per bucket, just
/// fewer, wider queries to get there. `generate_stream_jit_partitions_segment` itself is kept, not
/// called from here anymore -- see the module's "Keeping (and dropping) the segment functions"
/// notes.
#[span_fn]
pub async fn generate_stream_jit_partitions(
    config: &JitPartitionConfig,
    lakehouse: Arc<LakehouseContext>,
    blocks_view: &BlocksView,
    query_time_range: &TimeRange,
    stream: Arc<StreamMetadata>,
    process: Arc<ProcessMetadata>,
) -> Result<Vec<SourceDataBlocksInMemory>> {
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
    let insert_time_range = TimeRange::new(
        insert_time_range
            .begin
            .duration_trunc(config.max_insert_time_slice)?,
        insert_time_range
            .end
            .duration_trunc(config.max_insert_time_slice)?
            + config.max_insert_time_slice,
    );
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
    let bucket_counts = fetch_bucket_counts(
        lakehouse.clone(),
        blocks_view,
        &source_partitions,
        insert_time_range,
        config.max_insert_time_slice,
        &identity_predicate,
    )
    .await?;

    let windows: Vec<TimeRange> = batch_windows(
        insert_time_range,
        config.max_insert_time_slice,
        &bucket_counts,
        config.target_rows_per_query,
    )
    .collect();
    debug!(
        "generate_stream_jit_partitions: stream={stream_id}: derived {} batch window(s) from {} \
         non-empty bucket(s) (~{} row(s) total, target_rows_per_query={})",
        windows.len(),
        bucket_counts.len(),
        bucket_counts.iter().map(|(_, n)| n).sum::<i64>(),
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
        let reader_factory = lakehouse.reader_factory().clone();
        let df = instrument_named!(
            query_partitions(
                lakehouse.runtime().clone(),
                reader_factory,
                lakehouse.lake().blob_storage.inner(),
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
        for bucket_blocks in split_into_buckets(blocks, config.max_insert_time_slice)? {
            partitions.append(&mut group_blocks_into_partitions(config, bucket_blocks));
        }
    }
    Ok(partitions)
}

/// generate_process_jit_partitions lists the partitions that are needed to cover a time span for a specific process
/// these partitions may not exist or they could be out of date
/// Generates JIT partitions for a given time range filtered by process.
///
/// Batches its block queries the same way `generate_stream_jit_partitions` does, and additionally
/// applies the lean projection (see the module's "Why the lean projection is in scope" notes):
/// `fetch_stream_metadata_map` fetches every stream's metadata once for the whole insert range,
/// and each batch's `fetch_process_blocks` call looks it up per block instead of projecting stream
/// blobs onto every row and rebuilding `StreamMetadata` per row.
/// `generate_process_jit_partitions_segment` has been deleted -- its only caller was this function
/// -- and its grouping call is now inlined into the batch loop below; see the module's "Keeping
/// (and dropping) the segment functions" notes.
#[span_fn]
pub async fn generate_process_jit_partitions(
    config: &JitPartitionConfig,
    lakehouse: Arc<LakehouseContext>,
    blocks_view: &BlocksView,
    query_time_range: &TimeRange,
    process: Arc<ProcessMetadata>,
    stream_tag: &str,
) -> Result<Vec<SourceDataBlocksInMemory>> {
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

    let reader_factory = lakehouse.reader_factory().clone();
    let df = instrument_named!(
        query_partitions(
            lakehouse.runtime().clone(),
            reader_factory,
            lakehouse.lake().blob_storage.inner(),
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

    let insert_time_range = TimeRange::new(
        DateTime::from_timestamp_nanos(min_insert_time)
            .duration_trunc(config.max_insert_time_slice)?,
        DateTime::from_timestamp_nanos(max_insert_time)
            .duration_trunc(config.max_insert_time_slice)?
            + config.max_insert_time_slice,
    );

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
    let bucket_counts = fetch_bucket_counts(
        lakehouse.clone(),
        blocks_view,
        &source_partitions,
        insert_time_range,
        config.max_insert_time_slice,
        &identity_predicate,
    )
    .await?;

    let stream_metadata = fetch_stream_metadata_map(
        lakehouse.clone(),
        blocks_view,
        &source_partitions,
        insert_time_range,
        &process,
        stream_tag,
    )
    .await?;

    let windows: Vec<TimeRange> = batch_windows(
        insert_time_range,
        config.max_insert_time_slice,
        &bucket_counts,
        config.target_rows_per_query,
    )
    .collect();
    debug!(
        "generate_process_jit_partitions: process={process_id} stream_tag={stream_tag}: derived \
         {} batch window(s) from {} non-empty bucket(s) (~{} row(s) total, \
         target_rows_per_query={})",
        windows.len(),
        bucket_counts.len(),
        bucket_counts.iter().map(|(_, n)| n).sum::<i64>(),
        config.target_rows_per_query
    );

    let mut partitions = vec![];
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
        for bucket_blocks in split_into_buckets(blocks, config.max_insert_time_slice)? {
            partitions.append(&mut group_blocks_into_partitions(config, bucket_blocks));
        }
    }
    Ok(partitions)
}

/// One `lakehouse_partitions` candidate row, as fetched by `fetch_freshness_candidates`:
/// `(begin_insert_time, end_insert_time, file_schema_hash, source_data_hash)`. Fields are `pub` so
/// `analytics/tests/jit_freshness_tests.rs` can build rows directly, without a live database.
#[derive(Debug, Clone)]
pub struct PartitionFreshnessRow {
    pub begin_insert_time: DateTime<Utc>,
    pub end_insert_time: DateTime<Utc>,
    pub file_schema_hash: Vec<u8>,
    pub source_data_hash: Vec<u8>,
}

/// Fetches every `lakehouse_partitions` row for `view_meta` whose insert range *inclusively
/// overlaps* `range` (`begin_insert_time <= range.end AND end_insert_time >= range.begin`) -- a
/// superset of what any of the three `BlockOrder`-dependent per-spec queries used to return (an
/// exact-equality row necessarily overlaps the spec's own range), with the variant-specific
/// narrowing applied in Rust by `spec_is_up_to_date`. Used both by `is_jit_partition_up_to_date`
/// (one spec's own range) and `find_up_to_date_partitions` (one range spanning many specs).
async fn fetch_freshness_candidates(
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

/// Filters `candidates` down to the rows the per-spec SQL used to return (exact range equality for
/// `BlockOrder::EventTime`, exact match for a degenerate `BlockOrder::InsertTime` range, inclusive
/// overlap otherwise), then applies the same `rows.len() == 1` / `file_schema_hash` /
/// object-count checks `is_jit_partition_up_to_date` always has.
///
/// `block_order` selects which comparison applies (see `BlockOrder`):
/// - `BlockOrder::EventTime` (`thread_spans`/`net_spans` only) requires exact insert-range and
///   exact count equality. Their cut points can move between `jit_update` runs, so a later run's
///   spec can be narrower than an already-written partition that still overlaps it; the overlap/`>=`
///   test below would call that stale, wider partition up to date and leave it in place forever.
///   Exact equality reports "not up to date" instead, so the write falls through to
///   `RetireMatch::Overlap` (see `write_partition.rs`).
/// - `BlockOrder::InsertTime` (every other JIT view) keeps the original overlap/`>=`-count test
///   (with an exact-match branch for a degenerate spec range, to avoid matching multiple/wider
///   rows). Their cut points are stable across runs, so a stale spec already covered by a wider
///   committed partition -- e.g. from a concurrent `jit_update` that lost a race -- must stay a
///   no-op here: `RetireMatch::Containment` cannot retire a merely-overlapping partition, so
///   calling such a spec "not up to date" would make the subsequent insert trip the
///   `lakehouse_partitions_no_overlap` exclusion constraint.
pub fn spec_is_up_to_date(
    view_meta: &ViewMetadata,
    spec: &SourceDataBlocksInMemory,
    block_order: BlockOrder,
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

    // See: https://github.com/madesroches/micromegas/issues/488
    let matches: Vec<&PartitionFreshnessRow> = candidates
        .iter()
        .filter(|r| match block_order {
            // Exact insert-range equality (see this function's docs for why).
            BlockOrder::EventTime => {
                r.begin_insert_time == min_insert_time && r.end_insert_time == max_insert_time
            }
            // Degenerate range: exact-match on the single insert time, to avoid matching
            // multiple/wider overlapping rows (see this function's docs).
            BlockOrder::InsertTime if min_insert_time == max_insert_time => {
                r.begin_insert_time == min_insert_time && r.end_insert_time == min_insert_time
            }
            // Overlap test, using inclusive inequalities (<=, >=) to prevent race conditions: with
            // exclusive inequalities (<, >), identical time ranges never match, causing partitions
            // to be unnecessarily recreated on every query (see this function's docs and
            // https://github.com/madesroches/micromegas/issues/488).
            BlockOrder::InsertTime => {
                r.begin_insert_time <= max_insert_time && r.end_insert_time >= min_insert_time
            }
        })
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
    let up_to_date = match block_order {
        // Exact count equality: see this function's docs.
        BlockOrder::EventTime => existing_count == required_count,
        // `>=`, not exact equality: a stale spec covered by an already-wider partition is a no-op
        // here, not a rewrite (see this function's docs).
        BlockOrder::InsertTime => existing_count >= required_count,
    };
    if !up_to_date {
        info!(
            "{desc}: existing partition object count does not satisfy block_order={block_order:?} \
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
/// matcher (`spec_is_up_to_date`) against it -- see that function's docs for the per-`BlockOrder`
/// semantics. Kept for the `BlockOrder::EventTime` callers (`net_spans`/`thread_spans`), whose
/// checks stay per-spec, interleaved with retire logic inside their own `update_partition`. The
/// five `BlockOrder::InsertTime` callers instead batch through `find_up_to_date_partitions`.
#[span_fn]
pub async fn is_jit_partition_up_to_date(
    pool: &sqlx::PgPool,
    view_meta: ViewMetadata,
    spec: &SourceDataBlocksInMemory,
    block_order: BlockOrder,
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
    spec_is_up_to_date(&view_meta, spec, block_order, &candidates)
}

/// One candidates fetch over `[specs.first().min, specs.last().max]` (specs have ascending,
/// non-overlapping insert ranges), then the matcher per spec, run to a fixpoint. An empty `specs`
/// returns `Ok(vec![])` without issuing any fetch -- reachable at every call site, since
/// `generate_*_jit_partitions` returns no specs when the range holds no blocks.
///
/// Round 1 runs `spec_is_up_to_date` for every spec against the full candidate set. Each later
/// round: for any spec `i` newly verdicted **not** up to date this round, drop from every other
/// spec `j`'s candidate set any row entirely contained in `i`'s insert range, and re-run the
/// matcher for those affected `j`s (see the module's "Verdicts reflect pre-run state" design
/// notes) -- such a row is a `RetireMatch::Containment` match for spec `i` and will be gone once
/// `i`'s write runs this call, so it must not count towards `j`'s freshness. Repeat until a round
/// flips no verdict: dropping a row can only turn a spec from up-to-date to stale, never the
/// reverse, so verdicts are monotone and the loop terminates in at most `specs.len()` rounds (one,
/// in the common case). A row is dropped only when its containing spec is itself stale; specs
/// whose containing spec is up to date (hence not rewritten) are unaffected. Returns up-to-date
/// flags parallel to `specs`.
#[span_fn]
pub async fn find_up_to_date_partitions(
    pool: &sqlx::PgPool,
    view_meta: ViewMetadata,
    block_order: BlockOrder,
    specs: &[SourceDataBlocksInMemory],
) -> Result<Vec<bool>> {
    if specs.is_empty() {
        return Ok(vec![]);
    }
    let ranges: Vec<(DateTime<Utc>, DateTime<Utc>)> = specs
        .iter()
        .map(get_part_insert_time_range)
        .collect::<Result<Vec<_>>>()?;
    let outer_range = TimeRange::new(
        ranges
            .first()
            .with_context(|| "find_up_to_date_partitions: empty ranges")?
            .0,
        ranges
            .last()
            .with_context(|| "find_up_to_date_partitions: empty ranges")?
            .1,
    );
    debug!(
        "find_up_to_date_partitions: fetching freshness candidates for {} spec(s) over [{}, {}]",
        specs.len(),
        outer_range.begin.to_rfc3339(),
        outer_range.end.to_rfc3339(),
    );
    let mut candidates = instrument_named!(
        fetch_freshness_candidates(pool, &view_meta, outer_range),
        "sql_select_freshness_candidates"
    )
    .await?;

    let mut up_to_date: Vec<bool> = specs
        .iter()
        .map(|spec| spec_is_up_to_date(&view_meta, spec, block_order, &candidates))
        .collect::<Result<Vec<_>>>()?;

    // Fixpoint: a spec verdicted not up to date will retire, this run, every candidate row
    // entirely contained in its insert range (`RetireMatch::Containment`); such a row must not
    // count towards a sibling spec's freshness. Verdicts are monotone (up-to-date -> stale only),
    // so this loop drops no more rows, and flips no more verdicts, than there are specs.
    loop {
        if !up_to_date.contains(&false) {
            break;
        }
        let stale_ranges: Vec<(DateTime<Utc>, DateTime<Utc>)> = up_to_date
            .iter()
            .zip(ranges.iter())
            .filter(|(up, _)| !**up)
            .map(|(_, r)| *r)
            .collect();
        let next_candidates: Vec<PartitionFreshnessRow> = candidates
            .iter()
            .filter(|row| {
                !stale_ranges.iter().any(|(min_i, max_i)| {
                    row.begin_insert_time >= *min_i && row.end_insert_time <= *max_i
                })
            })
            .cloned()
            .collect();
        let next_up_to_date: Vec<bool> = specs
            .iter()
            .map(|spec| spec_is_up_to_date(&view_meta, spec, block_order, &next_candidates))
            .collect::<Result<Vec<_>>>()?;
        if next_up_to_date == up_to_date {
            break;
        }
        candidates = next_candidates;
        up_to_date = next_up_to_date;
    }

    Ok(up_to_date)
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

/// Writes a partition from a set of blocks.
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
    };
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    block_spec
        .write(lake, null_response_writer)
        .await
        .with_context(|| "block_spec.write")?;
    Ok(())
}
