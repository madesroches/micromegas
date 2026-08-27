//! DB-gated test for the batched process-scoped JIT path
//! (tasks/completed/jit_batched_block_queries_plan.md, Testing Strategy): drives
//! `generate_process_jit_partitions` directly over a multi-bucket range with a lowered
//! `target_rows_per_query`, forcing the run to span more than one batch (not just more than one
//! bucket within a single batch), under **both** `BlockOrder` variants -- covering the batched
//! process path (including the `EventTime` batch-then-split path used in production by
//! `net_spans`) and the lean projection.
//!
//! The expected specs are computed by calling `fetch_process_blocks` once over the whole test
//! range -- the same fetch/parse code the rewritten generator's per-batch loop calls -- then
//! splitting the result into per-bucket runs and running `group_blocks_into_partitions` per
//! bucket. This shares the fetch/parse SQL path with the code under test instead of duplicating
//! it, so the assertion is not circular.
//!
//! Two streams are used so the per-stream metadata pre-query's `HashMap` lookup (the lean
//! projection) is exercised for more than one stream.
//!
//! Requires a live `MICROMEGAS_SQL_CONNECTION_STRING` / `MICROMEGAS_OBJECT_STORE_URI`; does not
//! run under a plain `cargo test`.

use anyhow::{Context, Result};
use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use micromegas_analytics::lakehouse::batch_update::regenerate_partition_range;
use micromegas_analytics::lakehouse::blocks_view::BlocksView;
use micromegas_analytics::lakehouse::jit_partitions::{
    BlockOrder, JitPartitionConfig, fetch_process_blocks, generate_process_jit_partitions,
    group_blocks_into_partitions,
};
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::PartitionCache;
use micromegas_analytics::lakehouse::partition_source_data::{
    PartitionSourceBlock, SourceDataBlocksInMemory,
};
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::view::View;
use micromegas_analytics::metadata::{StreamMetadata, find_process};
use micromegas_analytics::response_writer::{Logger, ResponseWriter};
use micromegas_analytics::time::TimeRange;
use micromegas_ingestion::data_lake_connection::connect_to_data_lake;
use micromegas_ingestion::web_ingestion_service::{FORMAT_TRANSIT, WebIngestionService};
use micromegas_ingestion::write_audience::WriteAudience;
use micromegas_telemetry::wire_format::encode_cbor;
use micromegas_telemetry_sink::TelemetryGuardBuilder;
use micromegas_telemetry_sink::stream_block::StreamBlock;
use micromegas_telemetry_sink::stream_info::make_stream_info;
use micromegas_tracing::dispatch::make_process_info;
use micromegas_tracing::event::TracingBlock;
use micromegas_tracing::levels::LevelFilter;
use micromegas_tracing::logs::{LogBlock, LogStaticStrInteropEvent, LogStream};
use micromegas_tracing::process_info::ProcessInfo;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use uuid::Uuid;

/// Pushes one `LogStaticStrInteropEvent` (1 object), closes the current block, and inserts it.
/// Mirrors `thread_spans_ordering_db_test.rs`'s `push_and_insert_block` for `ThreadStream`.
async fn push_and_insert_log_block(
    ingestion: &WebIngestionService,
    stream: &mut LogStream,
    process_info: &ProcessInfo,
    audience: &WriteAudience,
) -> Result<()> {
    stream.get_events_mut().push(LogStaticStrInteropEvent {
        time: 0,
        level: 2,
        target: "target".into(),
        msg: "msg".into(),
    });
    let next_offset = stream.get_block_ref().object_offset() + stream.get_block_ref().nb_objects();
    let mut block = stream.replace_block(Arc::new(LogBlock::new(
        1024,
        stream.process_id(),
        stream.stream_id(),
        next_offset,
    )));
    Arc::get_mut(&mut block)
        .with_context(|| "sole owner of freshly replaced block")?
        .close();
    let encoded = block.encode_bin(process_info)?;
    ingestion
        .insert_block(bytes::Bytes::from(encoded), audience)
        .await
        .map_err(|e| anyhow::anyhow!("insert_block: {e}"))?;
    Ok(())
}

/// Force-regenerates `blocks_view`'s bucket(s) covering `insert_range` (which must exactly tile
/// `TimeDelta::hours(1)`), bypassing the "already covered by an overlapping partition" freshness
/// check. See `thread_spans_ordering_db_test.rs`'s helper of the same name for the full rationale.
async fn regenerate_global_view(
    lakehouse: Arc<LakehouseContext>,
    view: Arc<dyn View>,
    insert_range: TimeRange,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let partitions = Arc::new(
        PartitionCache::fetch_overlapping_insert_range(&lakehouse.lake().db_pool, insert_range)
            .await?,
    );
    regenerate_partition_range(
        partitions,
        lakehouse,
        view,
        insert_range,
        TimeDelta::hours(1),
        logger,
    )
    .await?;
    Ok(())
}

/// See `thread_spans_ordering_db_test.rs`'s helper of the same name: process-global, one-time
/// telemetry setup, needed because the code under test uses `#[span_fn]`/`instrument_named!`.
fn ensure_telemetry_guard() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let guard = TelemetryGuardBuilder::default()
            .with_ctrlc_handling()
            .with_local_sink_max_level(LevelFilter::Info)
            .build()
            .expect("telemetry guard");
        std::mem::forget(guard);
    });
}

/// The "expected" computation the plan's Testing Strategy calls for: split an insert-time-sorted
/// block list (as `fetch_process_blocks` returns it) into per-bucket runs and run
/// `group_blocks_into_partitions` over each bucket independently -- what the batched generator's
/// per-batch loop does internally, just without the batching (one pass over every block).
fn expected_specs(
    config: &JitPartitionConfig,
    blocks: Vec<Arc<PartitionSourceBlock>>,
) -> Result<Vec<SourceDataBlocksInMemory>> {
    let mut buckets: BTreeMap<DateTime<Utc>, Vec<Arc<PartitionSourceBlock>>> = BTreeMap::new();
    for block in blocks {
        let bucket = block
            .block
            .insert_time
            .duration_trunc(config.max_insert_time_slice)?;
        buckets.entry(bucket).or_default().push(block);
    }
    let mut out = vec![];
    for (_, bucket_blocks) in buckets {
        out.append(&mut group_blocks_into_partitions(config, bucket_blocks));
    }
    Ok(out)
}

/// Asserts `actual` (the batched generator's output) and `expected` (the per-bucket, unbatched
/// computation above) emit the same specs: same block ids in the same order, same
/// `block_ids_hash`.
fn assert_specs_match(
    actual: &[SourceDataBlocksInMemory],
    expected: &[SourceDataBlocksInMemory],
    label: &str,
) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "{label}: expected {} spec(s), got {}",
        expected.len(),
        actual.len()
    );
    for (a, e) in actual.iter().zip(expected.iter()) {
        assert_eq!(
            a.block_ids_hash, e.block_ids_hash,
            "{label}: block_ids_hash mismatch"
        );
        let a_ids: Vec<_> = a.blocks.iter().map(|b| b.block.block_id).collect();
        let e_ids: Vec<_> = e.blocks.iter().map(|b| b.block.block_id).collect();
        assert_eq!(a_ids, e_ids, "{label}: block ids/order mismatch");
    }
}

#[ignore]
#[tokio::test]
async fn generate_process_jit_partitions_batched_matches_fetch_and_group() -> Result<()> {
    ensure_telemetry_guard();
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let lake = connect_to_data_lake(&connection_string, &object_store_uri).await?;
    let ingestion = WebIngestionService::new(lake.clone(), WriteAudience::new("public")?);
    let audience = WriteAudience::new("public")?;

    let process_id = Uuid::new_v4();
    let process_info = make_process_info(process_id, None, HashMap::new());
    let process_body = bytes::Bytes::from(encode_cbor(&process_info)?);
    ingestion
        .insert_process(process_body, &audience)
        .await
        .map_err(|e| anyhow::anyhow!("insert_process: {e}"))?;

    // Two log streams (tag "log", matching what generate_process_jit_partitions filters on),
    // alternating across 4 hour buckets: 1 block/bucket total, so a lowered
    // target_rows_per_query = 2 below packs 2 adjacent buckets into one batch, forcing more than
    // one batch over the range without forcing any cut *within* a bucket.
    let mut stream_a = LogStream::new(1024, process_id, &[String::from("log")], HashMap::new());
    let stream_a_id = stream_a.stream_id();
    let stream_a_info = make_stream_info(&stream_a);
    ingestion
        .insert_stream(bytes::Bytes::from(encode_cbor(&stream_a_info)?), &audience)
        .await
        .map_err(|e| anyhow::anyhow!("insert_stream: {e}"))?;

    let mut stream_b = LogStream::new(1024, process_id, &[String::from("log")], HashMap::new());
    let stream_b_id = stream_b.stream_id();
    let stream_b_info = make_stream_info(&stream_b);
    ingestion
        .insert_stream(bytes::Bytes::from(encode_cbor(&stream_b_info)?), &audience)
        .await
        .map_err(|e| anyhow::anyhow!("insert_stream: {e}"))?;

    push_and_insert_log_block(&ingestion, &mut stream_a, &process_info, &audience).await?;
    push_and_insert_log_block(&ingestion, &mut stream_b, &process_info, &audience).await?;
    push_and_insert_log_block(&ingestion, &mut stream_a, &process_info, &audience).await?;
    push_and_insert_log_block(&ingestion, &mut stream_b, &process_info, &audience).await?;

    // (stream_id, object_offset, begin_ticks, end_ticks, bucket_index) -- one block per bucket,
    // alternating stream_a/stream_b.
    let base_hour = (Utc::now() - TimeDelta::hours(10)).duration_trunc(TimeDelta::hours(1))?;
    let overrides = [
        (stream_a_id, 0i64, 0i64, 100i64, 0i64),
        (stream_b_id, 0, 100, 200, 1),
        (stream_a_id, 1, 200, 300, 2),
        (stream_b_id, 1, 300, 400, 3),
    ];
    for (stream_id, object_offset, begin_ticks, end_ticks, bucket_index) in overrides {
        let bucket_time = base_hour + TimeDelta::hours(bucket_index) + TimeDelta::minutes(30);
        sqlx::query(
            "UPDATE blocks SET begin_ticks = $1, end_ticks = $2, insert_time = $3, \
                                begin_time = $3, end_time = $3 \
             WHERE stream_id = $4 AND object_offset = $5",
        )
        .bind(begin_ticks)
        .bind(end_ticks)
        .bind(bucket_time)
        .bind(stream_id)
        .bind(object_offset)
        .execute(&lake.db_pool)
        .await
        .with_context(|| format!("overriding block stream={stream_id} offset={object_offset}"))?;
    }

    let lake = Arc::new(lake);
    let runtime = Arc::new(make_runtime_env()?);
    let lakehouse = Arc::new(LakehouseContext::new(lake.clone(), runtime.clone())?);
    let null_response_writer = Arc::new(ResponseWriter::new(None));

    let whole_range = TimeRange::new(base_hour, base_hour + TimeDelta::hours(4));
    let blocks_view = Arc::new(BlocksView::new(lakehouse.default_audience())?);
    regenerate_global_view(
        lakehouse.clone(),
        blocks_view.clone(),
        whole_range,
        null_response_writer.clone(),
    )
    .await?;

    let query_range = TimeRange::new(
        base_hour - TimeDelta::seconds(10),
        base_hour + TimeDelta::hours(4) + TimeDelta::seconds(10),
    );

    let process_meta = Arc::new(
        find_process(&lake.db_pool, &process_id, &lakehouse.default_audience())
            .await
            .with_context(|| "find_process")?,
    );
    // Built directly from the locally-known `StreamInfo`s -- no need to materialize a separate
    // `streams` view; `generate_process_jit_partitions` never queries it either (stream metadata
    // comes from the `streams.*` columns `BlocksView`'s own join already carries).
    let mut stream_metadata_map = HashMap::new();
    stream_metadata_map.insert(
        stream_a_id,
        (
            Arc::new(StreamMetadata::from_stream_info(&stream_a_info)?),
            FORMAT_TRANSIT.to_string(),
        ),
    );
    stream_metadata_map.insert(
        stream_b_id,
        (
            Arc::new(StreamMetadata::from_stream_info(&stream_b_info)?),
            FORMAT_TRANSIT.to_string(),
        ),
    );

    let source_partitions = PartitionCache::fetch_overlapping_insert_range_for_view(
        &lake.db_pool,
        blocks_view.get_view_set_name(),
        blocks_view.get_view_instance_id(),
        whole_range,
    )
    .await?;
    let all_blocks = fetch_process_blocks(
        lakehouse.clone(),
        &blocks_view,
        &source_partitions,
        &whole_range,
        process_meta.clone(),
        "log",
        &stream_metadata_map,
    )
    .await
    .with_context(|| "fetch_process_blocks")?;
    assert_eq!(all_blocks.len(), 4, "expected all 4 blocks to be fetched");

    for block_order in [BlockOrder::InsertTime, BlockOrder::EventTime] {
        let config = JitPartitionConfig {
            max_nb_objects: 1000,
            max_insert_time_slice: TimeDelta::hours(1),
            block_order,
            target_rows_per_query: 2,
        };

        let batched_specs = generate_process_jit_partitions(
            &config,
            lakehouse.clone(),
            &blocks_view,
            &query_range,
            process_meta.clone(),
            "log",
        )
        .await
        .with_context(|| format!("generate_process_jit_partitions {block_order:?}"))?;

        let expected = expected_specs(&config, all_blocks.clone())?;
        assert_specs_match(&batched_specs, &expected, &format!("{block_order:?}"));
    }

    Ok(())
}
