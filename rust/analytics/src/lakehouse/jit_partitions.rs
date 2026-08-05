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
//! `BlockOrder::EventTime`, where the natural size-based cut point is not always safe.

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
    metadata::{ProcessMetadata, StreamMetadata, block_from_batch_row},
    properties::properties_column_accessor::properties_column_by_name,
    response_writer::ResponseWriter,
    time::TimeRange,
};
use anyhow::{Context, Result};
use chrono::DurationRound;
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::arrow::array::{BinaryArray, GenericListArray, StringArray};
use datafusion::arrow::datatypes::{Schema, TimestampNanosecondType};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

/// How source blocks are ordered before being cut into JIT partitions.
///
/// The choice matters because `generate_stream_jit_partitions_segment` /
/// `generate_process_jit_partitions_segment` query blocks with `ORDER BY insert_time, block_id`,
/// and that SQL order is what `group_blocks_into_partitions` starts from.
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
}

impl Default for JitPartitionConfig {
    fn default() -> Self {
        JitPartitionConfig {
            max_nb_objects: 20 * 1024 * 1024,
            max_insert_time_slice: TimeDelta::hours(1),
            block_order: BlockOrder::InsertTime,
        }
    }
}

/// Returns the real `[min, max]` `insert_time` range covered by `blocks`, independent of list
/// order. Always use this instead of reading `blocks[0]`/`blocks[last]`: under
/// `BlockOrder::EventTime` the list is sorted by event time, not insert time, so its endpoints are
/// no longer the insert-time extremes.
pub fn insert_time_range(blocks: &[Arc<PartitionSourceBlock>]) -> Result<TimeRange> {
    let mut blocks_iter = blocks.iter();
    let first = blocks_iter
        .next()
        .with_context(|| "insert_time_range: empty block list")?;
    let mut min_insert_time = first.block.insert_time;
    let mut max_insert_time = first.block.insert_time;
    for block in blocks_iter {
        min_insert_time = min_insert_time.min(block.block.insert_time);
        max_insert_time = max_insert_time.max(block.block.insert_time);
    }
    Ok(TimeRange::new(min_insert_time, max_insert_time))
}

/// Emits one partition (if it holds any objects) from `blocks[start..end]`, warning once if the
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
    // Matches today's guard: a trailing window whose blocks all report nb_objects == 0 emits
    // nothing, not "if the block list is non-empty".
    if nb_objects == 0 {
        return;
    }
    if grown_past_limit > 0 {
        let process_id = blocks[start].process.process_id;
        let stream_id = blocks[start].stream.stream_id;
        warn!(
            "group_blocks_into_partitions: process={process_id} stream={stream_id} emitted a \
             partition of {nb_objects} objects (soft limit max_nb_objects={max_nb_objects}) after \
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
///   (and non-decreasing relative to) every other partition's.
///
/// Under `BlockOrder::InsertTime` both invariants hold for free: `blocks` is already sorted by
/// `insert_time`, so every candidate cut point is insert-safe and this function cuts at exactly
/// `max_nb_objects`, bit-identical to a naive greedy cut.
///
/// Under `BlockOrder::EventTime`, `blocks` is stable-sorted by `(begin_ticks, end_ticks)` first
/// (ties keep the incoming, insert-ordered position, so grouping stays deterministic); a cut
/// point is then only taken where it is *insert-safe*: every block already in the partition being
/// closed must have an `insert_time` no later than every remaining block's, computed via a
/// suffix-minimum of `insert_time` over the event-time-sorted list. When the natural
/// `max_nb_objects` cut point isn't safe, the cut looks back to the most recent safe index instead
/// (bounding the partition preceding an insert-time straggler); when no safe index exists at all
/// in the window (a straggler's own re-seeded window, or a continuous inversion chain), the window
/// grows past the soft limit and a `warn!` fires once the partition is finally emitted. See
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
    // branch warns about itself directly and does not feed this counter (its emitted prefix is
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
                let process_id = blocks[start].process.process_id;
                let stream_id = blocks[start].stream.stream_id;
                warn!(
                    "group_blocks_into_partitions: process={process_id} stream={stream_id} cut \
                     moved back from index {i} to {j} ({} block(s) looked back)",
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
                for k in start..i {
                    nb_objects += blocks[k].block.nb_objects as i64;
                    prefix_max_insert = prefix_max_insert.max(blocks[k].block.insert_time);
                    if prefix_max_insert <= suffix_min[k + 1] {
                        last_safe = Some((k + 1, nb_objects));
                    }
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
    let segment_source_partitions = instrument_named!(
        PartitionCache::fetch_overlapping_insert_range_for_view(
            &lakehouse.lake().db_pool,
            blocks_view.get_view_set_name(),
            blocks_view.get_view_instance_id(),
            insert_time_range,
        ),
        "fetch_overlapping_insert_range_for_view"
    )
    .await?;

    let mut begin_segment = insert_time_range.begin;
    let mut end_segment = begin_segment + config.max_insert_time_slice;
    let mut partitions = vec![];
    while end_segment <= insert_time_range.end {
        let insert_time_range = TimeRange::new(begin_segment, end_segment);
        let mut segment_partitions = generate_stream_jit_partitions_segment(
            config,
            lakehouse.clone(),
            blocks_view,
            &segment_source_partitions,
            &insert_time_range,
            stream.clone(),
            process.clone(),
        )
        .await?;
        partitions.append(&mut segment_partitions);
        begin_segment = end_segment;
        end_segment = begin_segment + config.max_insert_time_slice;
    }
    Ok(partitions)
}

/// Generates a segment of JIT partitions filtered by process.
#[span_fn]
pub async fn generate_process_jit_partitions_segment(
    config: &JitPartitionConfig,
    lakehouse: Arc<LakehouseContext>,
    blocks_view: &BlocksView,
    partitions: &PartitionCache,
    insert_time_range: &TimeRange,
    process: Arc<ProcessMetadata>,
    stream_tag: &str,
) -> Result<Vec<SourceDataBlocksInMemory>> {
    let partitions = partitions
        .filter_insert_range(*insert_time_range)
        .partitions;

    let process_id = &process.process_id;
    let begin_range_iso = insert_time_range.begin.to_rfc3339();
    let end_range_iso = insert_time_range.end.to_rfc3339();
    let sql = format!(
        r#"SELECT block_id, stream_id, process_id, begin_time, end_time, begin_ticks, end_ticks, nb_objects, object_offset, payload_size, insert_time,
             "streams.dependencies_metadata", "streams.objects_metadata", "streams.tags", "streams.properties", "streams.format"
             FROM source
             WHERE process_id = '{process_id}'
             AND array_has( "streams.tags", '{stream_tag}' )
             AND insert_time >= '{begin_range_iso}'
             AND insert_time < '{end_range_iso}'
             ORDER BY insert_time, block_id;"#
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
    let rbs = instrument_named!(df.collect(), "collect_partition_blocks").await?;

    let mut blocks = vec![];

    for rb in rbs {
        for ir in 0..rb.num_rows() {
            let block = block_from_batch_row(&rb, ir).with_context(|| "block_from_batch_row")?;

            // Build StreamMetadata from the query results
            let stream_id_column = string_column_by_name(&rb, "stream_id")?;
            let stream_process_id_column = string_column_by_name(&rb, "process_id")?;
            let dependencies_metadata_column: &BinaryArray =
                typed_column_by_name(&rb, "streams.dependencies_metadata")?;
            let objects_metadata_column: &BinaryArray =
                typed_column_by_name(&rb, "streams.objects_metadata")?;
            let stream_tags_column: &GenericListArray<i32> =
                typed_column_by_name(&rb, "streams.tags")?;
            let stream_properties_accessor = properties_column_by_name(&rb, "streams.properties")?;
            let stream_format_column = string_column_by_name(&rb, "streams.format")?;

            let stream_id = Uuid::parse_str(stream_id_column.value(ir)?)
                .with_context(|| "parsing stream_id")?;
            let stream_process_id = Uuid::parse_str(stream_process_id_column.value(ir)?)
                .with_context(|| "parsing stream process_id")?;

            let dependencies_metadata = dependencies_metadata_column.value(ir);
            let objects_metadata = objects_metadata_column.value(ir);
            let stream_tags = stream_tags_column
                .value(ir)
                .as_any()
                .downcast_ref::<StringArray>()
                .with_context(|| "casting stream_tags")?
                .iter()
                .map(|item| String::from(item.unwrap_or_default()))
                .collect();

            // Get pre-serialized JSONB properties directly from accessor
            let stream_properties_jsonb = stream_properties_accessor.jsonb_value(ir)?;

            let stream = Arc::new(StreamMetadata {
                stream_id,
                process_id: stream_process_id,
                dependencies_metadata: ciborium::from_reader(dependencies_metadata)
                    .with_context(|| "decoding dependencies_metadata")?,
                objects_metadata: ciborium::from_reader(objects_metadata)
                    .with_context(|| "decoding objects_metadata")?,
                tags: stream_tags,
                properties: Arc::new(stream_properties_jsonb),
            });

            let format = stream_format_column.value(ir)?.to_string();

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

/// generate_process_jit_partitions lists the partitions that are needed to cover a time span for a specific process
/// these partitions may not exist or they could be out of date
/// Generates JIT partitions for a given time range filtered by process.
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

    let segment_source_partitions = instrument_named!(
        PartitionCache::fetch_overlapping_insert_range_for_view(
            &lakehouse.lake().db_pool,
            blocks_view.get_view_set_name(),
            blocks_view.get_view_instance_id(),
            insert_time_range,
        ),
        "fetch_overlapping_insert_range_for_view"
    )
    .await?;

    let mut begin_segment = insert_time_range.begin;
    let mut end_segment = begin_segment + config.max_insert_time_slice;
    let mut partitions = vec![];

    while end_segment <= insert_time_range.end {
        let insert_time_range = TimeRange::new(begin_segment, end_segment);
        let mut segment_partitions = generate_process_jit_partitions_segment(
            config,
            lakehouse.clone(),
            blocks_view,
            &segment_source_partitions,
            &insert_time_range,
            process.clone(),
            stream_tag,
        )
        .await?;
        partitions.append(&mut segment_partitions);
        begin_segment = end_segment;
        end_segment = begin_segment + config.max_insert_time_slice;
    }
    Ok(partitions)
}

/// is_jit_partition_up_to_date compares a partition spec with the partitions that exist to know if it should be recreated
/// Checks if a JIT partition is up to date.
///
/// `block_order` selects which query/comparison applies (see `BlockOrder`'s docs):
/// - `BlockOrder::EventTime` (`thread_spans`/`net_spans` only) uses exact insert-range-and-count
///   equality. Their cut points can move between `jit_update` runs (see
///   `group_blocks_into_partitions`'s docs and `tasks/1429_jit_event_time_block_ordering_plan.md`
///   §6), so a later run's spec can have a smaller, different insert range than an
///   already-written partition that still overlaps it; calling that stale, wider partition "up to
///   date" (as the overlap/`>=` test below would) would leave it in place forever. Exact equality
///   correctly calls it "not up to date" instead, so it falls through to `retire_partitions`'s
///   `RetireMatch::Overlap` arm (see `write_partition.rs`).
/// - `BlockOrder::InsertTime` (every other JIT view) uses the original overlap/`>=`-count test:
///   `begin_insert_time <= max_insert_time AND end_insert_time >= min_insert_time` (or, for a
///   degenerate `min_insert_time == max_insert_time` spec, an exact-match
///   `begin_insert_time = end_insert_time = min_insert_time`, to avoid matching multiple/wider
///   overlapping rows), "up to date" iff a matching partition's object count is at least the
///   spec's. Their cut points are stable
///   across runs, so a stale spec (e.g. from a concurrent `jit_update` that lost a race -- see
///   `RetireMatch::Overlap`'s "Concurrent writers" docs) legitimately already-covered by a wider,
///   already-committed partition must be treated as a no-op here: `RetireMatch::Containment`
///   (what these views use) cannot retire a partition that merely overlaps without being
///   contained, so if exact equality called that stale spec "not up to date", the subsequent
///   insert would trip the `lakehouse_partitions_no_overlap` exclusion constraint. Using exact
///   equality here unconditionally was issue 2 of the 1429 branch's third review round.
#[span_fn]
pub async fn is_jit_partition_up_to_date(
    pool: &sqlx::PgPool,
    view_meta: ViewMetadata,
    spec: &SourceDataBlocksInMemory,
    block_order: BlockOrder,
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
    let rows = match block_order {
        BlockOrder::EventTime => {
            // Exact insert-range equality (see this function's docs for why).
            instrument_named!(
                sqlx::query(
                    "SELECT file_schema_hash, source_data_hash
             FROM lakehouse_partitions
             WHERE view_set_name = $1
             AND view_instance_id = $2
             AND begin_insert_time = $3
             AND end_insert_time = $4
             ;",
                )
                .bind(&*view_meta.view_set_name)
                .bind(&*view_meta.view_instance_id)
                .bind(min_insert_time)
                .bind(max_insert_time)
                .fetch_all(pool),
                "sql_select_matching_partitions"
            )
            .await
            .with_context(|| "fetching matching partitions")?
        }
        BlockOrder::InsertTime if min_insert_time == max_insert_time => {
            // Degenerate range: exact-match on the single insert time, to avoid matching
            // multiple/wider overlapping rows (see this function's docs).
            instrument_named!(
                sqlx::query(
                    "SELECT file_schema_hash, source_data_hash
             FROM lakehouse_partitions
             WHERE view_set_name = $1
             AND view_instance_id = $2
             AND begin_insert_time = $3
             AND end_insert_time = $3
             ;",
                )
                .bind(&*view_meta.view_set_name)
                .bind(&*view_meta.view_instance_id)
                .bind(min_insert_time)
                .fetch_all(pool),
                "sql_select_matching_partitions"
            )
            .await
            .with_context(|| "fetching matching partitions")?
        }
        BlockOrder::InsertTime => {
            // Overlap test, using inclusive inequalities (<=, >=) to prevent race conditions: with
            // exclusive inequalities (<, >), identical time ranges never match, causing partitions
            // to be unnecessarily recreated on every query (see this function's docs and
            // https://github.com/madesroches/micromegas/issues/488).
            instrument_named!(
                sqlx::query(
                    "SELECT file_schema_hash, source_data_hash
             FROM lakehouse_partitions
             WHERE view_set_name = $1
             AND view_instance_id = $2
             AND begin_insert_time <= $3
             AND end_insert_time >= $4
             ;",
                )
                .bind(&*view_meta.view_set_name)
                .bind(&*view_meta.view_instance_id)
                .bind(max_insert_time)
                .bind(min_insert_time)
                .fetch_all(pool),
                "sql_select_matching_partitions"
            )
            .await
            .with_context(|| "fetching matching partitions")?
        }
    };
    if rows.len() != 1 {
        debug!("{desc}: found {} partitions (expected 1)", rows.len());
        for (i, row) in rows.iter().enumerate() {
            let part_file_schema: Vec<u8> = row.try_get("file_schema_hash")?;
            let part_source_data: Vec<u8> = row.try_get("source_data_hash")?;
            let source_row_count = hash_to_object_count(&part_source_data)?;
            debug!(
                "{desc}: partition {}: file_schema_hash={:?}, source_rows={}",
                i, part_file_schema, source_row_count
            );
        }
        info!("{desc}: found {} partitions", rows.len());
        return Ok(false);
    }
    let r = &rows[0];
    let part_file_schema: Vec<u8> = r.try_get("file_schema_hash")?;
    if part_file_schema != view_meta.file_schema_hash {
        // this is dangerous because we could be creating a new partition smaller than the old one, which is not supported.
        // let's make sure there is no old data loitering
        warn!("{desc}: found matching partition with different file schema");
        return Ok(false);
    }
    let part_source_data: Vec<u8> = r.try_get("source_data_hash")?;
    let existing_count = hash_to_object_count(&part_source_data)?;
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

/// get_event_time_range returns the time range covered by a partition spec
/// Returns the insert time range covered by a partition spec.
fn get_part_insert_time_range(
    spec: &SourceDataBlocksInMemory,
) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let range =
        insert_time_range(&spec.blocks).with_context(|| "empty partition should not exist")?;
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
    let insert_range =
        insert_time_range(&source_data.blocks).with_context(|| "insert_time_range")?;
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
