//! DB-backed regression test for the perfetto-export sort-elimination plan
//! (tasks/1297_perfetto_redundant_sort_plan.md): with the `ORDER BY` removed, `begin` must still
//! come back non-decreasing across a `thread_spans` view instance spanning more than one JIT
//! partition. Requires a live `MICROMEGAS_SQL_CONNECTION_STRING` / `MICROMEGAS_OBJECT_STORE_URI`
//! (see `histo_view_test.rs` / `sql_view_test.rs` for the same harness pattern); does not run
//! under a plain `cargo test`.

use anyhow::{Context, Result};
use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use datafusion::arrow::array::TimestampNanosecondArray;
use micromegas_analytics::dfext::typed_column::{
    get_single_row_primitive_value_by_name, typed_column_by_name,
};
use micromegas_analytics::lakehouse::batch_update::{
    materialize_partition_range, regenerate_partition_range,
};
use micromegas_analytics::lakehouse::blocks_view::BlocksView;
use micromegas_analytics::lakehouse::jit_partitions::{
    BlockOrder, JitPartitionConfig, generate_stream_jit_partitions_segment,
};
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::{LivePartitionProvider, PartitionCache};
use micromegas_analytics::lakehouse::processes_view::make_processes_view;
use micromegas_analytics::lakehouse::query::query;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::streams_view::make_streams_view;
use micromegas_analytics::lakehouse::thread_spans_view::{ThreadSpansView, update_partition};
use micromegas_analytics::lakehouse::view::{View, ViewMetadata};
use micromegas_analytics::lakehouse::view_factory::{ViewFactory, default_view_factory};
use micromegas_analytics::metadata::{find_process_with_latest_timing, find_stream_from_view};
use micromegas_analytics::response_writer::{Logger, ResponseWriter};
use micromegas_analytics::time::{TimeRange, make_time_converter_from_latest_timing};
use micromegas_ingestion::data_lake_connection::connect_to_data_lake;
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_telemetry::wire_format::encode_cbor;
use micromegas_telemetry_sink::TelemetryGuardBuilder;
use micromegas_telemetry_sink::stream_block::StreamBlock;
use micromegas_telemetry_sink::stream_info::make_stream_info;
use micromegas_tracing::dispatch::make_process_info;
use micromegas_tracing::event::TracingBlock;
use micromegas_tracing::levels::LevelFilter;
use micromegas_tracing::prelude::*;
use micromegas_tracing::process_info::ProcessInfo;
use micromegas_tracing::spans::{
    BeginThreadNamedSpanEvent, EndThreadNamedSpanEvent, SpanLocation, ThreadBlock, ThreadStream,
};
use micromegas_tracing::time::now;
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;

static SPAN_LOCATION: SpanLocation = SpanLocation {
    lod: Verbosity::Med,
    target: "target",
    module_path: "module_path",
    file: "thread_spans_ordering_db_test.rs",
    line: 1,
};

/// Pushes one begin/end span pair, closes the current block, and inserts it.
///
/// `replace_block` returns the *old* block (the one holding the events just pushed) and installs
/// a *new*, empty one for whatever comes next -- so the new block's `object_offset` must be
/// computed from the old block's own offset and object count (matching
/// `dispatch.rs::flush_thread_buffer`), not passed in for "this" block; passing an
/// externally-incrementing counter here would tag the wrong block; every call ends up with the
/// value ("this" one, offset 0) while it's actually the *next* extracted block that gets its
/// offset advanced.
async fn push_and_insert_block(
    ingestion: &WebIngestionService,
    stream: &mut ThreadStream,
    process_info: &ProcessInfo,
    name: &'static str,
) -> Result<()> {
    let t0 = now();
    stream.get_events_mut().push(BeginThreadNamedSpanEvent {
        thread_span_location: &SPAN_LOCATION,
        name: name.into(),
        time: t0,
    });
    stream.get_events_mut().push(EndThreadNamedSpanEvent {
        thread_span_location: &SPAN_LOCATION,
        name: name.into(),
        time: t0 + 1_000_000,
    });
    let next_offset = stream.get_block_ref().object_offset() + stream.get_block_ref().nb_objects();
    let mut block = stream.replace_block(Arc::new(ThreadBlock::new(
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
        .insert_block(bytes::Bytes::from(encoded))
        .await
        .map_err(|e| anyhow::anyhow!("insert_block: {e}"))?;
    Ok(())
}

/// Materializes a global view over `insert_range`. `ThreadSpansView::jit_update` looks up its
/// source blocks, stream, and process through the `blocks` / `streams` / `processes` global
/// views, and (like `histo_view_test.rs` / `sql_view_test.rs`) these are only kept up to date by
/// the maintenance daemon in production, so tests must materialize them explicitly.
async fn materialize_global_view(
    lakehouse: Arc<LakehouseContext>,
    view: Arc<dyn View>,
    insert_range: TimeRange,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    // All-views partition cache: the transform query for `view` may read from other views (e.g.
    // `processes`/`streams` read from the freshly written `blocks` partitions), so this must not
    // be scoped to `view`'s own view_set_name (see `materialize_range` in histo_view_test.rs /
    // sql_view_test.rs for the same pattern).
    let partitions = Arc::new(
        PartitionCache::fetch_overlapping_insert_range(&lakehouse.lake().db_pool, insert_range)
            .await?,
    );
    materialize_partition_range(
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

/// Force-regenerates a global view's bucket(s) covering `insert_range` (which must exactly tile
/// `TimeDelta::hours(1)`, matching `materialize_global_view`'s own bucket size), bypassing
/// `materialize_partition_range`'s "already covered by *an* (even if stale) overlapping
/// partition" freshness check. Needed to make a re-materialization after new source rows have
/// been added (rather than the initial, first-time materialization) actually pick them up.
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

/// Ensures the process-wide telemetry guard (ctrlc handler, global tracing subscriber) is
/// initialized exactly once for a `#[tokio::test]` in this file. `TelemetryGuardBuilder::build`
/// does process-global, one-time setup (`ctrlc::set_handler` allows exactly one handler; the
/// global tracing subscriber can only be installed once), and this file has more than one
/// DB-backed test, all of which can run in the same test binary process -- so only the first
/// caller actually builds and installs it. The guard is intentionally leaked (never dropped):
/// there is no natural per-test teardown point when initialization is process-wide, and the
/// process exits at the end of the test binary regardless.
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

#[ignore]
#[tokio::test]
async fn thread_spans_ordering_across_partitions() -> Result<()> {
    ensure_telemetry_guard();
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let lake = connect_to_data_lake(&connection_string, &object_store_uri).await?;
    let ingestion = WebIngestionService::new(lake.clone());

    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, None, HashMap::new());
    let process_body = bytes::Bytes::from(encode_cbor(&process_info)?);
    ingestion
        .insert_process(process_body)
        .await
        .map_err(|e| anyhow::anyhow!("insert_process: {e}"))?;

    let mut stream = ThreadStream::new(1024, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();
    let stream_info = make_stream_info(&stream);
    let stream_body = bytes::Bytes::from(encode_cbor(&stream_info)?);
    ingestion
        .insert_stream(stream_body)
        .await
        .map_err(|e| anyhow::anyhow!("insert_stream: {e}"))?;

    // Block 1: earlier in real (event) time.
    push_and_insert_block(&ingestion, &mut stream, &process_info, "span_a").await?;

    // `replace_block` captures the *new* (next) block's begin timestamp before the *old* block's
    // `.close()` runs (same order as `dispatch.rs::flush_thread_buffer`), so the block installed
    // immediately after block 1 begins microseconds *before* block 1's own end is recorded -- a
    // hairline overlap that (with tsc_frequency == 0 in this environment, forcing estimated tick
    // conversion) is enough to trip the §3 non-overlap guard on two otherwise-correctly-ordered
    // blocks. Sleep, then discard one throwaway "spacer" block so the block that actually holds
    // block 2's spans gets a begin timestamp captured well after block 1's end, giving a real gap.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let spacer_offset =
        stream.get_block_ref().object_offset() + stream.get_block_ref().nb_objects();
    let _spacer = stream.replace_block(Arc::new(ThreadBlock::new(
        1024,
        stream.process_id(),
        stream.stream_id(),
        spacer_offset,
    )));

    // Block 2: later in real (event) time.
    push_and_insert_block(&ingestion, &mut stream, &process_info, "span_b").await?;

    // Force the two blocks into different 1-hour JIT insert-time segments -- rather than waiting
    // a real hour -- by pushing block 1's insert_time back. This is the cheap alternative the
    // plan calls out ("block insert-times that deliberately span more than one 1-hour JIT
    // segment") for forcing a second partition.
    //
    // `begin_time`/`end_time` (not just `insert_time`) must move back together: `BlocksView`'s
    // own event-time bounds are `[min(begin_time), max(insert_time)]` (a documented rough edge --
    // see `blocks_view.rs`'s "todo: make more robust" note), so shifting `insert_time` alone would
    // invert that range and make the partition match no query. `begin_ticks`/`end_ticks` are left
    // untouched: those (not `begin_time`/`end_time`) are what `ThreadSpansView` converts into the
    // actual exported span `begin`/`end` values, and this test wants those to stay "now" so the
    // final query -- and its own non-decreasing-`begin` assertion -- can use a narrow time window.
    sqlx::query(
        "UPDATE blocks SET insert_time = insert_time - INTERVAL '2 hours', \
                            begin_time = begin_time - INTERVAL '2 hours', \
                            end_time = end_time - INTERVAL '2 hours' \
         WHERE stream_id = $1 AND object_offset = 0;",
    )
    .bind(stream_id)
    .execute(&lake.db_pool)
    .await
    .with_context(|| "pushing block 1's insert_time/begin_time/end_time back")?;

    let lake = Arc::new(lake);
    let runtime = Arc::new(make_runtime_env()?);
    let lakehouse = Arc::new(LakehouseContext::new(lake.clone(), runtime.clone()));
    let view_factory = Arc::new(default_view_factory(runtime.clone(), lake.clone()).await?);
    let part_provider = Arc::new(LivePartitionProvider::new(lake.db_pool.clone()));
    let null_response_writer = Arc::new(ResponseWriter::new(None));

    let insert_range = TimeRange::new(
        Utc::now() - TimeDelta::hours(3),
        Utc::now() + TimeDelta::minutes(5),
    );
    let blocks_view = Arc::new(BlocksView::new()?);
    materialize_global_view(
        lakehouse.clone(),
        blocks_view.clone(),
        insert_range,
        null_response_writer.clone(),
    )
    .await?;
    let blocks_only_factory = Arc::new(ViewFactory::new(vec![blocks_view]));
    let processes_view = Arc::new(
        make_processes_view(runtime.clone(), lake.clone(), blocks_only_factory.clone()).await?,
    );
    materialize_global_view(
        lakehouse.clone(),
        processes_view,
        insert_range,
        null_response_writer.clone(),
    )
    .await?;
    let streams_view =
        Arc::new(make_streams_view(runtime.clone(), lake.clone(), blocks_only_factory).await?);
    materialize_global_view(
        lakehouse.clone(),
        streams_view,
        insert_range,
        null_response_writer.clone(),
    )
    .await?;

    // Wide enough to cover block 1's shifted-back begin_time/end_time (used by
    // `get_insert_time_range`'s event-time filter) as well as block 2's real "now".
    let query_range = TimeRange::new(
        Utc::now() - TimeDelta::hours(3),
        Utc::now() + TimeDelta::minutes(1),
    );
    let stream_id_str = stream_id.to_string();

    // Triggers ThreadSpansView::jit_update as a side effect of the scan.
    let _ = query(
        lakehouse.clone(),
        part_provider.clone(),
        Some(query_range),
        &format!(r#"SELECT "begin", "end" FROM view_instance('thread_spans', '{stream_id_str}');"#),
        view_factory.clone(),
        Arc::new(NoOpSessionConfigurator),
        false,
    )
    .await?;

    let partition_count_answer = query(
        lakehouse.clone(),
        part_provider.clone(),
        None,
        &format!(
            "SELECT count(*) as c FROM list_partitions() \
             WHERE view_set_name = 'thread_spans' AND view_instance_id = '{stream_id_str}';"
        ),
        view_factory.clone(),
        Arc::new(NoOpSessionConfigurator),
        false,
    )
    .await?;
    let partition_count = get_single_row_primitive_value_by_name::<
        datafusion::arrow::datatypes::Int64Type,
    >(&partition_count_answer.record_batches, "c")?;
    assert!(
        partition_count >= 2,
        "expected the two blocks (2h apart in insert_time) to materialize into >= 2 partitions, got {partition_count}"
    );

    // Plan-shape check against the real, multi-partition, DB-backed scan: the production query
    // (`format_thread_spans_query`) always keeps `ORDER BY begin` -- the declared ordering is
    // meant to make that `ORDER BY` free, not to make the scan's output order well-defined with
    // no ordering requirement at all. (An earlier version of this test dropped `ORDER BY` here;
    // DataFusion then felt free to insert a plain `RepartitionExec` -- `RoundRobinBatch`, with no
    // downstream requirement to reassemble a single order -- ahead of the two-file scan, and the
    // resulting row order depended on which partition's file read finished first, occasionally
    // reordering the two partitions' rows. `EnforceSorting` only elides *redundant* sorts; it does
    // not make an omitted `ORDER BY` reappear.) So this checks the actual production shape: with
    // `ORDER BY begin` present, no `SortExec` should appear, since `DataSourceExec`'s declared
    // ordering already satisfies it.
    let ctx = micromegas_analytics::lakehouse::query::make_session_context(
        lakehouse.clone(),
        part_provider.clone(),
        Some(query_range),
        view_factory.clone(),
        Arc::new(NoOpSessionConfigurator),
        false,
    )
    .await?;
    let df = ctx
        .sql(&format!(
            r#"SELECT "begin" FROM view_instance('thread_spans', '{stream_id_str}') ORDER BY "begin";"#
        ))
        .await?;
    let plan = df.create_physical_plan().await?;
    let plan_str = datafusion::physical_plan::displayable(plan.as_ref())
        .indent(true)
        .to_string();
    assert!(
        !plan_str.contains("SortExec"),
        "expected the declared ordering to elide the ORDER BY's Sort node, got:\n{plan_str}"
    );

    // The regression check: with `ORDER BY begin` present (as in production), `begin` comes back
    // non-decreasing across the multi-partition scan.
    let answer = query(
        lakehouse,
        part_provider,
        Some(query_range),
        &format!(r#"SELECT "begin" FROM view_instance('thread_spans', '{stream_id_str}') ORDER BY "begin";"#),
        view_factory,
        Arc::new(NoOpSessionConfigurator),
        false,
    )
    .await?;

    let mut previous: Option<i64> = None;
    let mut total_rows = 0;
    for batch in &answer.record_batches {
        let begins: &TimestampNanosecondArray = typed_column_by_name(batch, "begin")?;
        for i in 0..begins.len() {
            let b = begins.value(i);
            if let Some(p) = previous {
                assert!(b >= p, "begin regressed across the scan: {b} < {p}");
            }
            previous = Some(b);
            total_rows += 1;
        }
    }
    assert!(
        total_rows >= 2,
        "expected at least 2 span rows (one per block), got {total_rows}"
    );

    Ok(())
}

/// New case (Testing Strategy, Integration #11): blocks registered out of event order within a
/// *single* JIT partition (unlike the test above, which forces two separate partitions across an
/// insert-time segment boundary). Block 1 (span_a) is registered first but pushed back in
/// insert_time so it ends up event-time *later* than block 2 (span_b), which is registered
/// second. This is the write-side regression this plan's insert-safe cut rule prevents (an
/// inversion straddling a cut point trips the `lakehouse_partitions_no_overlap` exclusion
/// constraint) and the read-side regression its event-time sort prevents (mis-ordered `begin`,
/// `perfetto_trace_chunks` failing loudly). Only `insert_time` is overridden here -- `begin_ticks`/
/// `end_ticks`/`begin_time`/`end_time` (and the encoded span payloads) are left as actually
/// recorded, so call-tree decoding and tick-to-time frequency estimation both stay correct (the
/// latter derives its estimate from the *real* `begin_time`/`end_time` of the process's blocks;
/// moving those into the past relative to the process's own start_time makes the estimate
/// nonsensical).
#[ignore]
#[tokio::test]
async fn thread_spans_reversed_registration_survives_jit_update() -> Result<()> {
    ensure_telemetry_guard();
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let lake = connect_to_data_lake(&connection_string, &object_store_uri).await?;
    let ingestion = WebIngestionService::new(lake.clone());

    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, None, HashMap::new());
    let process_body = bytes::Bytes::from(encode_cbor(&process_info)?);
    ingestion
        .insert_process(process_body)
        .await
        .map_err(|e| anyhow::anyhow!("insert_process: {e}"))?;

    let mut stream = ThreadStream::new(1024, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();
    let stream_info = make_stream_info(&stream);
    let stream_body = bytes::Bytes::from(encode_cbor(&stream_info)?);
    ingestion
        .insert_stream(stream_body)
        .await
        .map_err(|e| anyhow::anyhow!("insert_stream: {e}"))?;

    // Block 1 (object_offset 0): registered first. Block 2 (object_offset 2): registered second.
    push_and_insert_block(&ingestion, &mut stream, &process_info, "span_a").await?;
    push_and_insert_block(&ingestion, &mut stream, &process_info, "span_b").await?;

    // Swap insert_time (only -- ticks and payloads are untouched) so block 2 (registered second)
    // is insert-time *earlier* than block 1 (registered first): an inversion straddling this
    // single partition, the ~10% case from the plan's Overview. Both timestamps stay safely in
    // the past (not just before "now" at override time, but before the materialization below's
    // insert_range, since `materialize_partition_range` only ever emits *whole* hourly buckets --
    // a value at or after that range's upper edge would silently fall in an unmaterialized
    // trailing partial bucket and never reach blocks_view at all).
    let t0 = Utc::now() - TimeDelta::seconds(30);
    sqlx::query("UPDATE blocks SET insert_time = $1 WHERE stream_id = $2 AND object_offset = 0")
        .bind(t0 + TimeDelta::seconds(5))
        .bind(stream_id)
        .execute(&lake.db_pool)
        .await
        .with_context(|| "pushing block 1's insert_time later")?;
    sqlx::query("UPDATE blocks SET insert_time = $1 WHERE stream_id = $2 AND object_offset = 2")
        .bind(t0)
        .bind(stream_id)
        .execute(&lake.db_pool)
        .await
        .with_context(|| "pushing block 2's insert_time earlier")?;

    let lake = Arc::new(lake);
    let runtime = Arc::new(make_runtime_env()?);
    let lakehouse = Arc::new(LakehouseContext::new(lake.clone(), runtime.clone()));
    let view_factory = Arc::new(default_view_factory(runtime.clone(), lake.clone()).await?);
    let part_provider = Arc::new(LivePartitionProvider::new(lake.db_pool.clone()));
    let null_response_writer = Arc::new(ResponseWriter::new(None));

    // A single, calendar-hour-aligned bucket (rather than materialize_global_view's
    // now-relative, non-exact-tiling range): using `regenerate_global_view` here, forcibly
    // rewriting from source regardless of what a previous test run in this same shared,
    // persistent dev lake already materialized for this hour, makes this test's own stream
    // reliably show up in blocks_view/processes_view/streams_view even when another DB test in
    // this file already touched the same hour bucket moments earlier (materialize_global_view's
    // plain "not up to date, abort" safety check would otherwise silently skip the update and
    // leave this stream undiscoverable).
    let hour_start = Utc::now().duration_trunc(TimeDelta::hours(1))?;
    let insert_range = TimeRange::new(hour_start, hour_start + TimeDelta::hours(1));
    let blocks_view = Arc::new(BlocksView::new()?);
    regenerate_global_view(
        lakehouse.clone(),
        blocks_view.clone(),
        insert_range,
        null_response_writer.clone(),
    )
    .await?;
    let blocks_only_factory = Arc::new(ViewFactory::new(vec![blocks_view]));
    let processes_view = Arc::new(
        make_processes_view(runtime.clone(), lake.clone(), blocks_only_factory.clone()).await?,
    );
    regenerate_global_view(
        lakehouse.clone(),
        processes_view,
        insert_range,
        null_response_writer.clone(),
    )
    .await?;
    let streams_view =
        Arc::new(make_streams_view(runtime.clone(), lake.clone(), blocks_only_factory).await?);
    regenerate_global_view(
        lakehouse.clone(),
        streams_view,
        insert_range,
        null_response_writer.clone(),
    )
    .await?;

    let query_range = TimeRange::new(
        Utc::now() - TimeDelta::hours(3),
        Utc::now() + TimeDelta::minutes(1),
    );
    let stream_id_str = stream_id.to_string();
    let process_id_str = process_id.to_string();

    // jit_update must succeed (no exclusion-constraint error) -- triggered as a side effect of the
    // scan below.
    query(
        lakehouse.clone(),
        part_provider.clone(),
        Some(query_range),
        &format!(r#"SELECT "begin", "end" FROM view_instance('thread_spans', '{stream_id_str}');"#),
        view_factory.clone(),
        Arc::new(NoOpSessionConfigurator),
        false,
    )
    .await
    .with_context(|| "jit_update via thread_spans scan must not trip the exclusion constraint")?;

    // begin comes back non-decreasing.
    let answer = query(
        lakehouse.clone(),
        part_provider.clone(),
        Some(query_range),
        &format!(
            r#"SELECT "begin" FROM view_instance('thread_spans', '{stream_id_str}') ORDER BY "begin";"#
        ),
        view_factory.clone(),
        Arc::new(NoOpSessionConfigurator),
        false,
    )
    .await?;
    let mut previous: Option<i64> = None;
    let mut total_rows = 0;
    for batch in &answer.record_batches {
        let begins: &TimestampNanosecondArray = typed_column_by_name(batch, "begin")?;
        for i in 0..begins.len() {
            let b = begins.value(i);
            if let Some(p) = previous {
                assert!(b >= p, "begin regressed across the scan: {b} < {p}");
            }
            previous = Some(b);
            total_rows += 1;
        }
    }
    assert!(
        total_rows >= 2,
        "expected at least 2 span rows, got {total_rows}"
    );

    // perfetto_trace_chunks completes without "thread spans out of order". Select an actual
    // column (not `count(*)`): a bare `count(*)` over this table function's zero-column
    // projection trips an unrelated DataFusion physical/logical schema-mismatch bug in this
    // version, independent of thread_spans ordering.
    let perfetto_answer = query(
        lakehouse.clone(),
        part_provider.clone(),
        None,
        &format!(
            "SELECT chunk_id FROM perfetto_trace_chunks('{process_id_str}', 'thread', \
             TIMESTAMP '{}', TIMESTAMP '{}');",
            (Utc::now() - TimeDelta::minutes(15)).to_rfc3339(),
            (Utc::now() + TimeDelta::minutes(15)).to_rfc3339(),
        ),
        view_factory.clone(),
        Arc::new(NoOpSessionConfigurator),
        false,
    )
    .await
    .with_context(|| "perfetto_trace_chunks must not fail with 'thread spans out of order'")?;
    let chunk_count: usize = perfetto_answer
        .record_batches
        .iter()
        .map(|b| b.num_rows())
        .sum();
    assert!(
        chunk_count > 0,
        "expected at least one perfetto trace chunk"
    );

    // list_partitions shows non-overlapping ranges, and their union covers both blocks.
    let list_answer = query(
        lakehouse,
        part_provider,
        None,
        &format!(
            "SELECT begin_insert_time, end_insert_time, min_event_time, max_event_time \
             FROM list_partitions() \
             WHERE view_set_name = 'thread_spans' AND view_instance_id = '{stream_id_str}' \
             ORDER BY begin_insert_time;"
        ),
        view_factory,
        Arc::new(NoOpSessionConfigurator),
        false,
    )
    .await?;
    let mut prev_end_insert: Option<i64> = None;
    let mut partitions_seen = 0;
    for batch in &list_answer.record_batches {
        let begin_insert: &TimestampNanosecondArray =
            typed_column_by_name(batch, "begin_insert_time")?;
        let end_insert: &TimestampNanosecondArray = typed_column_by_name(batch, "end_insert_time")?;
        for i in 0..begin_insert.len() {
            let b = begin_insert.value(i);
            let e = end_insert.value(i);
            if let Some(prev) = prev_end_insert {
                assert!(b >= prev, "insert-time ranges overlap: {b} < {prev}");
            }
            prev_end_insert = Some(e);
            partitions_seen += 1;
        }
    }
    assert!(
        partitions_seen >= 1,
        "expected at least one thread_spans partition"
    );

    Ok(())
}

/// Cross-run regrouping case (Design §6, "Cross-run stability"): a block that arrives between two
/// `jit_update` runs can legally shift an *earlier* cut point, leaving an already-written partition
/// that only *overlaps* (does not contain, and is not contained by) the newer, narrower range a
/// later run would produce. The second run must replace that stale partition instead of leaving
/// both in place. `ThreadSpansView::jit_update` hardcodes `JitPartitionConfig::default()`, so this
/// drives `generate_stream_jit_partitions_segment` directly with a lowered `max_nb_objects` to
/// force multiple partitions out of a handful of blocks, and `thread_spans_view::update_partition`
/// directly to write each spec without going through the view's `jit_update`.
#[ignore]
#[tokio::test]
async fn thread_spans_cross_run_regrouping_replaces_stale_partition() -> Result<()> {
    ensure_telemetry_guard();
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let lake = connect_to_data_lake(&connection_string, &object_store_uri).await?;
    let ingestion = WebIngestionService::new(lake.clone());

    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, None, HashMap::new());
    let process_body = bytes::Bytes::from(encode_cbor(&process_info)?);
    ingestion
        .insert_process(process_body)
        .await
        .map_err(|e| anyhow::anyhow!("insert_process: {e}"))?;

    let mut stream = ThreadStream::new(1024, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();
    let stream_info = make_stream_info(&stream);
    let stream_body = bytes::Bytes::from(encode_cbor(&stream_info)?);
    ingestion
        .insert_stream(stream_body)
        .await
        .map_err(|e| anyhow::anyhow!("insert_stream: {e}"))?;

    // Push 4 blocks (2 objects each) for run 1.
    for name in ["span_0", "span_1", "span_2", "span_3"] {
        push_and_insert_block(&ingestion, &mut stream, &process_info, name).await?;
    }

    // Force a deterministic event order (matching object_offset order: 0, 2, 4, 6) and
    // insert-time order (0s, 1s, 2s, 3s) for run 1 -- no inversions, so run 1's grouping is the
    // naive greedy cut: two 2-block partitions. `begin_time`/`end_time` are deliberately left
    // untouched (real): `make_time_converter_from_latest_timing` derives its tick-frequency
    // estimate from the *real* last block's `begin_time`/`end_time` relative to the process's
    // real `start_time`, and moving those into the past relative to `start_time` makes the
    // estimate negative ("invalid frequency"). `begin_ticks`/`end_ticks` and `insert_time` are
    // decoupled from wall-clock time entirely, so overriding just those is safe.
    let t0 = Utc::now() - TimeDelta::minutes(20);
    for (object_offset, begin_ticks, end_ticks, insert_secs) in [
        (0i64, 0i64, 1000i64, 0i64),
        (2, 1000, 2000, 1),
        (4, 2000, 3000, 2),
        (6, 3000, 4000, 3),
    ] {
        sqlx::query(
            "UPDATE blocks SET begin_ticks = $1, end_ticks = $2, insert_time = $3 \
             WHERE stream_id = $4 AND object_offset = $5",
        )
        .bind(begin_ticks)
        .bind(end_ticks)
        .bind(t0 + TimeDelta::seconds(insert_secs))
        .bind(stream_id)
        .bind(object_offset)
        .execute(&lake.db_pool)
        .await
        .with_context(|| format!("overriding block at object_offset {object_offset}"))?;
    }

    let lake = Arc::new(lake);
    let runtime = Arc::new(make_runtime_env()?);
    let lakehouse = Arc::new(LakehouseContext::new(lake.clone(), runtime.clone()));
    let view_factory = Arc::new(default_view_factory(runtime.clone(), lake.clone()).await?);
    let null_response_writer = Arc::new(ResponseWriter::new(None));

    // A single range covering both t0's hour and "now"'s hour (t0 is at most 20 minutes before
    // "now", so 2 hours from t0's truncated hour always reaches "now" too), truncated to
    // microsecond precision (Postgres's timestamptz columns only store microseconds, so a
    // boundary computed here at full nanosecond precision would never exactly equal the same
    // boundary read back from a stored partition) and exactly tiling `TimeDelta::hours(1)`.
    // `regenerate_global_view` (not `materialize_global_view`) forcibly rewrites from source
    // regardless of what a previous test run in this same shared, persistent dev lake already
    // materialized for these hours -- `materialize_global_view`'s plain "not up to date, abort"
    // safety check would otherwise silently skip the update and leave this stream's blocks (or,
    // after block 4 is added below, block 4 itself) undiscoverable.
    let materialize_begin = t0.duration_trunc(TimeDelta::hours(1))?;
    let materialize_range =
        TimeRange::new(materialize_begin, materialize_begin + TimeDelta::hours(2));
    let blocks_view = Arc::new(BlocksView::new()?);
    regenerate_global_view(
        lakehouse.clone(),
        blocks_view.clone(),
        materialize_range,
        null_response_writer.clone(),
    )
    .await?;
    let blocks_only_factory = Arc::new(ViewFactory::new(vec![blocks_view.clone()]));
    let processes_view = Arc::new(
        make_processes_view(runtime.clone(), lake.clone(), blocks_only_factory.clone()).await?,
    );
    regenerate_global_view(
        lakehouse.clone(),
        processes_view,
        materialize_range,
        null_response_writer.clone(),
    )
    .await?;
    let streams_view =
        Arc::new(make_streams_view(runtime.clone(), lake.clone(), blocks_only_factory).await?);
    regenerate_global_view(
        lakehouse.clone(),
        streams_view,
        materialize_range,
        null_response_writer.clone(),
    )
    .await?;

    let full_range = TimeRange::new(t0 - TimeDelta::seconds(10), t0 + TimeDelta::seconds(10));

    let stream_meta = Arc::new(
        find_stream_from_view(lakehouse.clone(), view_factory.clone(), &stream_id, None).await?,
    );
    let (process_meta, last_block_end_ticks, last_block_end_time) =
        find_process_with_latest_timing(lakehouse.clone(), view_factory.clone(), &process_id, None)
            .await?;
    let process_meta = Arc::new(process_meta);
    let convert_ticks = make_time_converter_from_latest_timing(
        &process_meta,
        last_block_end_ticks,
        last_block_end_time,
    )?;

    let thread_spans_view = ThreadSpansView::new(&stream_id.to_string(), view_factory.clone())?;
    let view_meta = ViewMetadata {
        view_set_name: thread_spans_view.get_view_set_name(),
        view_instance_id: thread_spans_view.get_view_instance_id(),
        file_schema_hash: thread_spans_view.get_file_schema_hash(),
    };
    let schema = thread_spans_view.get_file_schema();

    // 2 objects/block, so max_nb_objects=4 forces exactly 2 blocks/partition when there is no
    // insert-safety issue.
    let config = JitPartitionConfig {
        max_nb_objects: 4,
        max_insert_time_slice: TimeDelta::hours(1),
        block_order: BlockOrder::EventTime,
    };

    // Run 1: only blocks 0-3 exist yet.
    let segment_partitions_1 = PartitionCache::fetch_overlapping_insert_range_for_view(
        &lake.db_pool,
        blocks_view.get_view_set_name(),
        blocks_view.get_view_instance_id(),
        full_range,
    )
    .await?;
    let run1_specs = generate_stream_jit_partitions_segment(
        &config,
        lakehouse.clone(),
        &blocks_view,
        &segment_partitions_1,
        &full_range,
        stream_meta.clone(),
        process_meta.clone(),
    )
    .await?;
    assert_eq!(
        run1_specs.len(),
        2,
        "expected 2 partitions from blocks 0-3 with max_nb_objects=4"
    );
    for spec in &run1_specs {
        update_partition(
            lake.clone(),
            view_meta.clone(),
            schema.clone(),
            &convert_ticks,
            spec,
        )
        .await
        .with_context(|| "writing run 1 partition")?;
    }

    // Push block 4 and give it an insert_time that sorts into the *middle* of the event-time
    // order (between blocks 1 and 2), straddling run 1's cut point at index 2 -- the scenario
    // Design §6 describes as legally shifting an earlier cut point.
    push_and_insert_block(&ingestion, &mut stream, &process_info, "span_4").await?;
    sqlx::query(
        "UPDATE blocks SET begin_ticks = 1500, end_ticks = 1600, insert_time = $1 \
         WHERE stream_id = $2 AND object_offset = 8",
    )
    .bind(t0 + TimeDelta::milliseconds(2500))
    .bind(stream_id)
    .execute(&lake.db_pool)
    .await
    .with_context(|| "overriding block 4")?;

    // Re-materialize blocks_view so it picks up the new row: `materialize_global_view` would
    // find the bucket it already wrote for run 1 "overlapping" and abort rather than pick up
    // block 4, so force regeneration again instead.
    regenerate_global_view(
        lakehouse.clone(),
        blocks_view.clone(),
        materialize_range,
        null_response_writer.clone(),
    )
    .await?;

    let segment_partitions_2 = PartitionCache::fetch_overlapping_insert_range_for_view(
        &lake.db_pool,
        blocks_view.get_view_set_name(),
        blocks_view.get_view_instance_id(),
        full_range,
    )
    .await?;
    let run2_specs = generate_stream_jit_partitions_segment(
        &config,
        lakehouse.clone(),
        &blocks_view,
        &segment_partitions_2,
        &full_range,
        stream_meta,
        process_meta,
    )
    .await?;
    assert_eq!(
        run2_specs.len(),
        3,
        "expected 3 partitions after block 4 shifts the cut point"
    );
    for spec in &run2_specs {
        update_partition(
            lake.clone(),
            view_meta.clone(),
            schema.clone(),
            &convert_ticks,
            spec,
        )
        .await
        .with_context(|| "writing run 2 partition")?;
    }

    // The stale run-1 partition must be replaced, not left alongside: exactly 3 partitions
    // remain (run 2's), and their insert ranges are non-overlapping.
    let mut tr = lake.db_pool.begin().await?;
    let rows = sqlx::query(
        "SELECT begin_insert_time, end_insert_time FROM lakehouse_partitions \
         WHERE view_set_name = 'thread_spans' AND view_instance_id = $1 \
         ORDER BY begin_insert_time;",
    )
    .bind(stream_id.to_string())
    .fetch_all(&mut *tr)
    .await?;
    tr.commit().await?;
    assert_eq!(
        rows.len(),
        3,
        "run 2 must replace the stale wider partition, not leave both (found {} partitions)",
        rows.len()
    );
    let mut prev_end: Option<DateTime<Utc>> = None;
    for row in &rows {
        let b: DateTime<Utc> = row.try_get("begin_insert_time")?;
        let e: DateTime<Utc> = row.try_get("end_insert_time")?;
        if let Some(prev) = prev_end {
            assert!(b >= prev, "insert-time ranges overlap: {b} < {prev}");
        }
        prev_end = Some(e);
    }

    Ok(())
}
