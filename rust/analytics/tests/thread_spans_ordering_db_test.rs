//! DB-backed regression test for the perfetto-export sort-elimination plan
//! (tasks/1297_perfetto_redundant_sort_plan.md): with the `ORDER BY` removed, `begin` must still
//! come back non-decreasing across a `thread_spans` view instance spanning more than one JIT
//! partition. Requires a live `MICROMEGAS_SQL_CONNECTION_STRING` / `MICROMEGAS_OBJECT_STORE_URI`
//! (see `histo_view_test.rs` / `sql_view_test.rs` for the same harness pattern); does not run
//! under a plain `cargo test`.

use anyhow::{Context, Result};
use chrono::{TimeDelta, Utc};
use datafusion::arrow::array::TimestampNanosecondArray;
use micromegas_analytics::dfext::typed_column::{
    get_single_row_primitive_value_by_name, typed_column_by_name,
};
use micromegas_analytics::lakehouse::batch_update::materialize_partition_range;
use micromegas_analytics::lakehouse::blocks_view::BlocksView;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::{LivePartitionProvider, PartitionCache};
use micromegas_analytics::lakehouse::processes_view::make_processes_view;
use micromegas_analytics::lakehouse::query::query;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::streams_view::make_streams_view;
use micromegas_analytics::lakehouse::view::View;
use micromegas_analytics::lakehouse::view_factory::{ViewFactory, default_view_factory};
use micromegas_analytics::response_writer::{Logger, ResponseWriter};
use micromegas_analytics::time::TimeRange;
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

#[ignore]
#[tokio::test]
async fn thread_spans_ordering_across_partitions() -> Result<()> {
    let _telemetry_guard = TelemetryGuardBuilder::default()
        .with_ctrlc_handling()
        .with_local_sink_max_level(LevelFilter::Info)
        .build();
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
