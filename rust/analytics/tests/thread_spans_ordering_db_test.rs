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
use micromegas_analytics::lakehouse::batch_update::regenerate_partition_range;
use micromegas_analytics::lakehouse::blocks_view::BlocksView;
use micromegas_analytics::lakehouse::jit_partitions::{
    BlockOrder, JitPartitionConfig, generate_stream_jit_partitions,
    generate_stream_jit_partitions_segment,
};
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::{LivePartitionProvider, PartitionCache};
use micromegas_analytics::lakehouse::partition_source_data::{
    PartitionSourceBlock, SourceDataBlocksInMemory,
};
use micromegas_analytics::lakehouse::processes_view::make_processes_view;
use micromegas_analytics::lakehouse::query::query;
use micromegas_analytics::lakehouse::read_scope::CallerContext;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::streams_view::make_streams_view;
use micromegas_analytics::lakehouse::thread_spans_view::{ThreadSpansView, update_partition};
use micromegas_analytics::lakehouse::view::{View, ViewMetadata};
use micromegas_analytics::lakehouse::view_factory::{ViewFactory, default_view_factory};
use micromegas_analytics::lakehouse::write_partition::{RetireMatch, retire_partitions};
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
    push_pairs_and_insert_block(ingestion, stream, process_info, name, 1).await
}

/// Pushes `n_pairs` begin/end span pairs (so `2 * n_pairs` objects) into the current block, closes
/// it, and inserts it; `push_and_insert_block` is the `n_pairs == 1` case. The extra parameter lets
/// tests give sibling blocks distinct object counts (see
/// `thread_spans_same_run_consecutive_degenerate_siblings_survive`, which needs this to keep
/// `is_jit_partition_up_to_date`'s count comparison from mistaking one sibling for another).
async fn push_pairs_and_insert_block(
    ingestion: &WebIngestionService,
    stream: &mut ThreadStream,
    process_info: &ProcessInfo,
    name: &'static str,
    n_pairs: usize,
) -> Result<()> {
    for _ in 0..n_pairs {
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
    }
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

/// Retires every partition of `view` that *overlaps* `insert_range`, then regenerates the range
/// from source.
///
/// `regenerate_global_view` alone is not enough on a shared, persistent dev lake:
/// `regenerate_partition_range` refuses a bucket that does not *fully contain* each existing
/// partition it would replace, so a single partition straddling a bucket boundary -- e.g. one an
/// older, non-hour-aligned version of a test left behind, or one written with a different
/// partition delta -- fails the run with "regeneration bucket ... does not fully contain existing
/// partition ...". Retiring by overlap first (`RetireMatch::Overlap`, which unlike
/// `RetireMatch::Containment` matches a partition that merely straddles the boundary) makes the
/// subsequent regeneration independent of whatever shape the lake happened to be in. Global
/// metadata views are derived from Postgres tables, so discarding and rebuilding them is free of
/// data loss.
async fn reset_global_view(
    lakehouse: Arc<LakehouseContext>,
    view: Arc<dyn View>,
    insert_range: TimeRange,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let mut tr = lakehouse.lake().db_pool.begin().await?;
    retire_partitions(
        &mut tr,
        &view.get_view_set_name(),
        &view.get_view_instance_id(),
        insert_range.begin,
        insert_range.end,
        RetireMatch::Overlap,
        &[],
        logger.clone(),
    )
    .await
    .with_context(|| "retiring overlapping partitions before regeneration")?;
    tr.commit().await.with_context(|| "commit")?;
    regenerate_global_view(lakehouse, view, insert_range, logger).await
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

    // Block 2: later in real (event) time. No manufactured gap here -- `replace_block` (called
    // inside `push_and_insert_block`) captures the *new* (next) block's begin timestamp before
    // the *old* block's `.close()` runs, exactly the legacy overlap this plan's Part A fixes for
    // the production `dispatch.rs` flush paths (though not for this test's own direct
    // `ThreadStream`/`replace_block` usage, which deliberately keeps exercising the legacy
    // strictly-overlapping shape -- see the module doc). An earlier version of this test slept
    // 200ms and discarded a throwaway "spacer" block here to manufacture a real gap between block
    // 1's end and block 2's begin; deleting that workaround is this test's regression signal for
    // Part B's `max_sort_key_time`, which is what makes a real block-boundary overlap safe for the
    // `Concatenated` scan check without needing the gap at all.
    push_and_insert_block(&ingestion, &mut stream, &process_info, "span_b").await?;

    // Force the two blocks into different 1-hour JIT insert-time segments -- rather than waiting
    // a real hour -- by pushing block 1's insert_time back. This is the cheap alternative the
    // plan calls out ("block insert-times that deliberately span more than one 1-hour JIT
    // segment") for forcing a second partition.
    //
    // `begin_time`/`end_time` (not just `insert_time`) must move back together: `BlocksView`'s
    // own event-time bounds are `[min(begin_time), max(insert_time)]` (a documented rough edge --
    // see `blocks_view.rs`'s "todo: make more robust" note), so shifting `insert_time` alone would
    // invert that range and make the partition match no query. Block 1's `begin_ticks`/`end_ticks`
    // are left untouched: those (not `begin_time`/`end_time`) are what `ThreadSpansView` converts
    // into the actual exported span `begin`/`end` values, and this test wants those to stay "now"
    // so the final query -- and its own non-decreasing-`begin` assertion -- can use a narrow time
    // window.
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

    // Make the *legacy* (`max_event_time`-only) comparison's overlap deterministic rather than
    // inheriting the buffer-swap's hairline (and possibly microsecond-truncated) width, so that a
    // future revert of Part B's fix is guaranteed to be caught by this test rather than passing by
    // accident. Widen block 1's `end_ticks` forward by a few milliseconds' worth of ticks, computed
    // from this process's own observed tick rate (real tsc frequency if available, otherwise the
    // same wall-clock-elapsed estimate `make_time_converter_from_latest_timing` falls back to).
    //
    // This targets `end_ticks`, not `begin_ticks`, and block 1, not block 2 -- unlike the
    // wholesale tick-fabrication pattern the module doc warns against, but also deliberately unlike
    // widening block 2's `begin_ticks` backward, which looks equally plausible but is unsound here:
    // block 2's `begin_ticks` and block 1's row's own `begin` (`max_sort_key_time`, read from the
    // real, un-fabricated event payload) are both stamped from the *same* real-time neighborhood
    // (block 2's replacement is created immediately after span_a's events are pushed, before block
    // 1 is closed), so pushing block 2's `begin_ticks` back by milliseconds risks dragging it below
    // `max_sort_key_time`, breaking the very check this test means to exercise. Block 1's
    // `end_ticks`, by contrast, feeds only the legacy `max_event_time` bound -- `max_sort_key_time`
    // is computed from the row data alone and never reads it -- so widening it forward is inert to
    // the fixed check and free to be as large as needed to make the legacy comparison's failure
    // reliable. It is safe by the same two properties the module doc calls out for the
    // begin_ticks-lowering pattern: no real event is filtered out (growing `end_range_ns` only
    // widens the chain's `[begin_range_ns, end_range_ns]` window), and the block cannot invert
    // (`end_ticks` only increases here, and it already exceeds `begin_ticks`).
    let now_ticks = now();
    let now_time = Utc::now();
    let elapsed_ticks = now_ticks - process_info.start_ticks;
    let elapsed_ns = (now_time - process_info.start_time)
        .num_nanoseconds()
        .filter(|&ns| ns > 0)
        .with_context(|| "process elapsed wall time must be positive")?;
    #[allow(clippy::cast_precision_loss)]
    let delta_ticks = ((elapsed_ticks as f64) * (5_000_000.0 / elapsed_ns as f64)).round() as i64;
    anyhow::ensure!(
        delta_ticks > 0,
        "computed a non-positive tick delta ({delta_ticks}); the block-boundary overlap would not \
         be widened"
    );
    sqlx::query(
        "UPDATE blocks SET end_ticks = end_ticks + $1 WHERE stream_id = $2 AND object_offset = 0;",
    )
    .bind(delta_ticks)
    .bind(stream_id)
    .execute(&lake.db_pool)
    .await
    .with_context(|| "widening block 1's end_ticks to make the legacy overlap deterministic")?;

    let lake = Arc::new(lake);
    let runtime = Arc::new(make_runtime_env()?);
    let lakehouse = Arc::new(LakehouseContext::new(lake.clone(), runtime.clone()));
    let view_factory = Arc::new(default_view_factory(runtime.clone(), lake.clone()).await?);
    let part_provider = Arc::new(LivePartitionProvider::new(lake.db_pool.clone()));
    let null_response_writer = Arc::new(ResponseWriter::new(None));

    // Hour-aligned and exactly tiling `TimeDelta::hours(1)`, as `regenerate_global_view`
    // requires: starting from the hour containing `now - 3h`, five buckets always reach past
    // `now`. `regenerate_global_view` (not `materialize_global_view`) forcibly rewrites from
    // source regardless of what a previous test -- in this file or any other sharing this
    // persistent dev lake -- already materialized for these hours. With
    // `materialize_global_view`, its "found overlapping partition, aborting the update" freshness
    // check silently skips the update whenever those buckets already exist, leaving this test's
    // freshly ingested stream undiscoverable and failing the run with
    // "find_stream_from_view: Stream not found". The sibling tests below already use
    // `regenerate_global_view` for the same reason.
    let insert_begin = (Utc::now() - TimeDelta::hours(3)).duration_trunc(TimeDelta::hours(1))?;
    let insert_range = TimeRange::new(insert_begin, insert_begin + TimeDelta::hours(5));
    let blocks_view = Arc::new(BlocksView::new()?);
    reset_global_view(
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
    reset_global_view(
        lakehouse.clone(),
        processes_view,
        insert_range,
        null_response_writer.clone(),
    )
    .await?;
    let streams_view =
        Arc::new(make_streams_view(runtime.clone(), lake.clone(), blocks_only_factory).await?);
    reset_global_view(
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
        CallerContext::internal(),
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
        CallerContext::internal(),
    )
    .await?;
    let partition_count = get_single_row_primitive_value_by_name::<
        datafusion::arrow::datatypes::Int64Type,
    >(&partition_count_answer.record_batches, "c")?;
    assert!(
        partition_count >= 2,
        "expected the two blocks (2h apart in insert_time) to materialize into >= 2 partitions, got {partition_count}"
    );

    // The regression assertion this test exists to prove: every partition the query scanned
    // actually persisted a non-NULL `max_sort_key_time` -- the only thing that makes the
    // block-boundary overlap harmless to the `Concatenated` non-overlap check without the
    // sleep/spacer workaround this test used to need. This is what proves `update_partition`
    // actually persists the new column and that the write path populated it, not just that the
    // scan happened to succeed.
    let null_sort_key_time_answer = query(
        lakehouse.clone(),
        part_provider.clone(),
        None,
        &format!(
            "SELECT count(*) as c FROM list_partitions() \
             WHERE view_set_name = 'thread_spans' AND view_instance_id = '{stream_id_str}' \
             AND max_sort_key_time IS NULL;"
        ),
        view_factory.clone(),
        Arc::new(NoOpSessionConfigurator),
        false,
    )
    .await?;
    let null_sort_key_time_count = get_single_row_primitive_value_by_name::<
        datafusion::arrow::datatypes::Int64Type,
    >(&null_sort_key_time_answer.record_batches, "c")?;
    assert_eq!(
        null_sort_key_time_count, 0,
        "every thread_spans partition written by this test must carry a non-NULL max_sort_key_time"
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
        CallerContext::internal(),
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
        CallerContext::internal(),
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
        CallerContext::internal(),
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
        CallerContext::internal(),
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
        CallerContext::internal(),
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
        CallerContext::internal(),
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

/// Builds a hand-picked `SourceDataBlocksInMemory` spec from a slice of already-fetched blocks,
/// bypassing `group_blocks_into_partitions`'s automatic cutting so the tests below can construct
/// exact partition boundaries (in particular, degenerate ranges and shared insert-time boundaries)
/// that would otherwise require reverse-engineering the cut algorithm's size/safety interplay.
/// `block_ids_hash` follows the same encoding `emit_partition` uses (`nb_objects.to_le_bytes()`).
fn slice_spec(blocks: &[Arc<PartitionSourceBlock>]) -> SourceDataBlocksInMemory {
    let nb_objects: i64 = blocks.iter().map(|b| b.block.nb_objects as i64).sum();
    SourceDataBlocksInMemory {
        blocks: blocks.to_vec(),
        block_ids_hash: nb_objects.to_le_bytes().to_vec(),
    }
}

/// Degenerate-range retirement (Design §6, `write_partition.rs`'s `RetireMatch::Overlap`):
/// a *new* partition whose insert range is degenerate (`begin_insert_time == end_insert_time`)
/// must still retire a stale, wider partition from a prior run that overlaps it. The predicate's
/// inclusive `'[]'` bounds make this work: with Postgres's default half-open bounds
/// `tstzrange(t, t)` is empty, so a plain `&&` would match zero rows for every degenerate new
/// range. Writes a wide
/// "run 1" partition covering all 3 blocks, then a degenerate "run 2" partition consisting of just
/// the middle block (whose insert_time sits strictly inside run 1's range) and asserts the wide
/// partition is gone, leaving only the new degenerate one.
#[ignore]
#[tokio::test]
async fn thread_spans_degenerate_range_retires_stale_partition() -> Result<()> {
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

    // Push 3 blocks (2 objects each).
    for name in ["span_0", "span_1", "span_2"] {
        push_and_insert_block(&ingestion, &mut stream, &process_info, name).await?;
    }

    // Deterministic event order (matching object_offset order: 0, 2, 4) and insert-time order
    // (0s, 1s, 2s) -- no inversions, so blocks[1] (the middle one) legitimately sits strictly
    // inside the [0s, 2s] range once all 3 are grouped into one partition.
    let t0 = Utc::now() - TimeDelta::minutes(20);
    for (object_offset, begin_ticks, end_ticks, insert_secs) in [
        (0i64, 0i64, 1000i64, 0i64),
        (2, 1000, 2000, 1),
        (4, 2000, 3000, 2),
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

    // Large max_nb_objects: no splitting, all 3 blocks land in one event-ordered spec.
    let config = JitPartitionConfig {
        max_nb_objects: 1000,
        max_insert_time_slice: TimeDelta::hours(1),
        block_order: BlockOrder::EventTime,
        target_rows_per_query: 250_000,
    };
    let segment_partitions = PartitionCache::fetch_overlapping_insert_range_for_view(
        &lake.db_pool,
        blocks_view.get_view_set_name(),
        blocks_view.get_view_instance_id(),
        full_range,
    )
    .await?;
    let specs = generate_stream_jit_partitions_segment(
        &config,
        lakehouse.clone(),
        &blocks_view,
        &segment_partitions,
        &full_range,
        stream_meta.clone(),
        process_meta.clone(),
    )
    .await?;
    assert_eq!(
        specs.len(),
        1,
        "expected all 3 blocks in a single spec with a large max_nb_objects"
    );
    let all_blocks = &specs[0].blocks;
    assert_eq!(all_blocks.len(), 3, "expected all 3 blocks in the spec");

    // "Run 1": write a stale, wide partition covering all 3 blocks (insert range [t0, t0+2s]).
    // Each simulated run gets its own fresh same_run_ranges accumulator, since they are not the
    // same jit_update loop.
    let wide_spec = slice_spec(all_blocks);
    let mut run1_same_run_ranges: Vec<TimeRange> = Vec::new();
    update_partition(
        lake.clone(),
        view_meta.clone(),
        schema.clone(),
        &convert_ticks,
        &wide_spec,
        &mut run1_same_run_ranges,
    )
    .await
    .with_context(|| "writing run 1 wide partition")?;

    // "Run 2": regrouping now writes just the middle block on its own -- a degenerate partition
    // (begin_insert_time == end_insert_time == t0+1s) whose point sits strictly inside run 1's
    // range. This is exactly the bug's scenario: with half-open range bounds, this write would
    // retire nothing and the stale wide partition would survive alongside the new one.
    let degenerate_spec = slice_spec(&all_blocks[1..2]);
    let mut run2_same_run_ranges: Vec<TimeRange> = Vec::new();
    update_partition(
        lake.clone(),
        view_meta.clone(),
        schema.clone(),
        &convert_ticks,
        &degenerate_spec,
        &mut run2_same_run_ranges,
    )
    .await
    .with_context(|| "writing run 2 degenerate partition")?;

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
        1,
        "the degenerate new range must retire the stale wide partition, leaving only the new \
         degenerate one (found {} partitions)",
        rows.len()
    );
    let b: DateTime<Utc> = rows[0].try_get("begin_insert_time")?;
    let e: DateTime<Utc> = rows[0].try_get("end_insert_time")?;
    assert_eq!(
        b, e,
        "the surviving partition should be the new degenerate one"
    );

    Ok(())
}

/// Same-run left-boundary exclusion (Design §6, "The containment half's left-boundary
/// exclusion"): a legal cut can land a degenerate partition (here, a single block) immediately
/// before a following partition, written in the *same* run, that begins at that exact same
/// insert-time point. Writing the second partition must not retire its own just-written,
/// immediately-preceding sibling. Asserts both partitions survive.
#[ignore]
#[tokio::test]
async fn thread_spans_same_run_left_boundary_survives() -> Result<()> {
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

    // Push 3 blocks (2 objects each).
    for name in ["span_0", "span_1", "span_2"] {
        push_and_insert_block(&ingestion, &mut stream, &process_info, name).await?;
    }

    // Blocks 0 and 1 share the exact same insert_time `t`; block 2 is later. Event order matches
    // object_offset order (0, 2, 4), so grouping all 3 into one spec and slicing block 0 alone
    // from blocks 1+2 reproduces the shape the cut rule can legally produce: a degenerate
    // partition ending at `t`, immediately followed by a partition that begins at that same `t`.
    let t0 = Utc::now() - TimeDelta::minutes(20);
    for (object_offset, begin_ticks, end_ticks, insert_secs) in [
        (0i64, 0i64, 1000i64, 0i64),
        (2, 1000, 2000, 0i64),
        (4, 2000, 3000, 1i64),
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

    let config = JitPartitionConfig {
        max_nb_objects: 1000,
        max_insert_time_slice: TimeDelta::hours(1),
        block_order: BlockOrder::EventTime,
        target_rows_per_query: 250_000,
    };
    let segment_partitions = PartitionCache::fetch_overlapping_insert_range_for_view(
        &lake.db_pool,
        blocks_view.get_view_set_name(),
        blocks_view.get_view_instance_id(),
        full_range,
    )
    .await?;
    let specs = generate_stream_jit_partitions_segment(
        &config,
        lakehouse.clone(),
        &blocks_view,
        &segment_partitions,
        &full_range,
        stream_meta.clone(),
        process_meta.clone(),
    )
    .await?;
    assert_eq!(
        specs.len(),
        1,
        "expected all 3 blocks in a single spec with a large max_nb_objects"
    );
    let all_blocks = &specs[0].blocks;
    assert_eq!(all_blocks.len(), 3, "expected all 3 blocks in the spec");

    // Partition A: block 0 alone -- degenerate, insert range [t, t].
    let partition_a = slice_spec(&all_blocks[0..1]);
    // Partition B: blocks 1+2 -- begins at that same `t` (block 1's insert_time), ends later.
    let partition_b = slice_spec(&all_blocks[1..3]);

    // Both partitions are written by the same simulated jit_update run, so they share one
    // same_run_ranges accumulator -- this is exactly what protects A from B's retire step.
    let mut same_run_ranges: Vec<TimeRange> = Vec::new();
    update_partition(
        lake.clone(),
        view_meta.clone(),
        schema.clone(),
        &convert_ticks,
        &partition_a,
        &mut same_run_ranges,
    )
    .await
    .with_context(|| "writing degenerate partition A")?;
    update_partition(
        lake.clone(),
        view_meta.clone(),
        schema.clone(),
        &convert_ticks,
        &partition_b,
        &mut same_run_ranges,
    )
    .await
    .with_context(|| "writing partition B")?;

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
        2,
        "partition B's write must not retire its own just-written, degenerate, \
         immediately-preceding sibling (found {} partitions)",
        rows.len()
    );
    let b0: DateTime<Utc> = rows[0].try_get("begin_insert_time")?;
    let e0: DateTime<Utc> = rows[0].try_get("end_insert_time")?;
    let b1: DateTime<Utc> = rows[1].try_get("begin_insert_time")?;
    assert_eq!(b0, e0, "partition A should still be degenerate");
    assert_eq!(
        e0, b1,
        "partition B should begin exactly where degenerate partition A ends"
    );

    Ok(())
}

/// Interrupted-run reconvergence (Design §6, "Why the `Overlap` waiver is tolerable"): a
/// `jit_update` loop that errors out or is cancelled after writing only some of a multi-partition
/// regrouping spec must not leave a permanent gap. Writes only the first of a 2-partition spec
/// (simulating the interrupted loop), then recomputes the same grouping from the unchanged blocks
/// and writes every partition in it (simulating the next, successful `jit_update`): the first
/// partition is recognized as already up to date and skipped, the second is a fresh insert, and
/// the result covers the full insert range with no gap and no overlap.
#[ignore]
#[tokio::test]
async fn thread_spans_interrupted_run_reconverges() -> Result<()> {
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

    // Push 4 blocks (2 objects each).
    for name in ["span_0", "span_1", "span_2", "span_3"] {
        push_and_insert_block(&ingestion, &mut stream, &process_info, name).await?;
    }

    // Deterministic event order and insert order (0s, 1s, 2s, 3s) -- no inversions. Truncated to
    // microsecond precision (Postgres's timestamptz columns only store microseconds), since the
    // final assertions below compare a stored, read-back boundary against `t0` for exact
    // equality.
    let t0 = (Utc::now() - TimeDelta::minutes(20)).duration_trunc(TimeDelta::microseconds(1))?;
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

    // 2 objects/block, so max_nb_objects=4 forces exactly 2 blocks/partition.
    let config = JitPartitionConfig {
        max_nb_objects: 4,
        max_insert_time_slice: TimeDelta::hours(1),
        block_order: BlockOrder::EventTime,
        target_rows_per_query: 250_000,
    };
    let segment_partitions = PartitionCache::fetch_overlapping_insert_range_for_view(
        &lake.db_pool,
        blocks_view.get_view_set_name(),
        blocks_view.get_view_instance_id(),
        full_range,
    )
    .await?;
    let specs = generate_stream_jit_partitions_segment(
        &config,
        lakehouse.clone(),
        &blocks_view,
        &segment_partitions,
        &full_range,
        stream_meta.clone(),
        process_meta.clone(),
    )
    .await?;
    assert_eq!(
        specs.len(),
        2,
        "expected 2 partitions from blocks 0-3 with max_nb_objects=4"
    );

    // Simulate an interrupted jit_update loop: only the first partition gets written.
    let mut interrupted_same_run_ranges: Vec<TimeRange> = Vec::new();
    update_partition(
        lake.clone(),
        view_meta.clone(),
        schema.clone(),
        &convert_ticks,
        &specs[0],
        &mut interrupted_same_run_ranges,
    )
    .await
    .with_context(|| "writing only the first partition (simulating an interrupted run)")?;

    // A full jit_update: blocks are unchanged, so grouping is deterministic and recomputes the
    // same 2-partition spec. The first partition is recognized as already up to date (skipped);
    // the second is a fresh insert.
    let segment_partitions_2 = PartitionCache::fetch_overlapping_insert_range_for_view(
        &lake.db_pool,
        blocks_view.get_view_set_name(),
        blocks_view.get_view_instance_id(),
        full_range,
    )
    .await?;
    let reconverge_specs = generate_stream_jit_partitions_segment(
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
        reconverge_specs.len(),
        2,
        "grouping is deterministic: the follow-up run must recompute the same 2 partitions"
    );
    // The follow-up run is itself one jit_update loop, so its partitions share one accumulator
    // (distinct from the interrupted run's, above -- they are different runs).
    let mut followup_same_run_ranges: Vec<TimeRange> = Vec::new();
    for spec in &reconverge_specs {
        update_partition(
            lake.clone(),
            view_meta.clone(),
            schema.clone(),
            &convert_ticks,
            spec,
            &mut followup_same_run_ranges,
        )
        .await
        .with_context(|| "writing follow-up run partition")?;
    }

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
        2,
        "expected exactly 2 partitions after reconvergence (found {})",
        rows.len()
    );
    let b0: DateTime<Utc> = rows[0].try_get("begin_insert_time")?;
    let e0: DateTime<Utc> = rows[0].try_get("end_insert_time")?;
    let b1: DateTime<Utc> = rows[1].try_get("begin_insert_time")?;
    let e1: DateTime<Utc> = rows[1].try_get("end_insert_time")?;
    assert!(
        b1 >= e0,
        "insert-time ranges must not overlap: [{b0}, {e0}] then [{b1}, {e1}]"
    );
    // No missing tail range: the union of both partitions' insert ranges covers every pushed
    // block (insert times 0s..3s from t0), i.e. the follow-up run's second partition genuinely
    // covers blocks 2-3 rather than the interrupted run's first partition being silently left as
    // the only surviving data.
    assert_eq!(
        b0, t0,
        "the first partition (from the interrupted run) must still start at the earliest block's insert time"
    );
    assert_eq!(
        e1,
        t0 + TimeDelta::seconds(3),
        "the follow-up run's completion must reach the latest block's insert time, leaving no \
         missing tail range: [{b0}, {e0}] then [{b1}, {e1}]"
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
        target_rows_per_query: 250_000,
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
    let mut run1_same_run_ranges: Vec<TimeRange> = Vec::new();
    for spec in &run1_specs {
        update_partition(
            lake.clone(),
            view_meta.clone(),
            schema.clone(),
            &convert_ticks,
            spec,
            &mut run1_same_run_ranges,
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
    // Run 2 is a distinct jit_update loop from run 1, so it gets its own fresh accumulator: run
    // 1's partitions are not "same run" for run 2's writes, and the stale one among them must
    // still be retired.
    let mut run2_same_run_ranges: Vec<TimeRange> = Vec::new();
    for spec in &run2_specs {
        update_partition(
            lake.clone(),
            view_meta.clone(),
            schema.clone(),
            &convert_ticks,
            spec,
            &mut run2_same_run_ranges,
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

/// Cross-run degenerate-predecessor case (review round 3, issue 1): a stale *degenerate*
/// partition from an earlier run must still be retired when a later run's regrouping produces a
/// single, wider, non-degenerate partition that starts at the same insert time -- the ordinary
/// "single-block partition grows as more blocks arrive" case. This is the scenario the
/// containment arm's since-removed left-boundary exclusion used to break: the new range's
/// `begin_insert_time` equals the stale degenerate partition's (only) insert time, which used to
/// trip `NOT (begin_insert_time = end_insert_time AND begin_insert_time = $3)` and leave the
/// stale partition in place, duplicating its one block's rows across two partitions. Writes run
/// 1 (a single block, alone -- a degenerate `[t0, t0]` partition) and, in a separate simulated
/// run with its own fresh `same_run_ranges`, run 2 (the same block plus two more, regrouped into
/// one non-degenerate `[t0, t0+2s]` partition) and asserts run 1's partition is gone.
#[ignore]
#[tokio::test]
async fn thread_spans_cross_run_degenerate_predecessor_retired_by_growing_partition() -> Result<()>
{
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

    // Only block 0 exists for run 1.
    push_and_insert_block(&ingestion, &mut stream, &process_info, "span_0").await?;

    // Truncated to microsecond precision (Postgres's timestamptz columns only store
    // microseconds), since the final assertion compares a stored, read-back boundary against
    // `t0` for exact equality.
    let t0 = (Utc::now() - TimeDelta::minutes(20)).duration_trunc(TimeDelta::microseconds(1))?;
    sqlx::query(
        "UPDATE blocks SET begin_ticks = 0, end_ticks = 1000, insert_time = $1 \
         WHERE stream_id = $2 AND object_offset = 0",
    )
    .bind(t0)
    .bind(stream_id)
    .execute(&lake.db_pool)
    .await
    .with_context(|| "overriding block 0")?;

    let lake = Arc::new(lake);
    let runtime = Arc::new(make_runtime_env()?);
    let lakehouse = Arc::new(LakehouseContext::new(lake.clone(), runtime.clone()));
    let view_factory = Arc::new(default_view_factory(runtime.clone(), lake.clone()).await?);
    let null_response_writer = Arc::new(ResponseWriter::new(None));

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

    let config = JitPartitionConfig {
        max_nb_objects: 1000,
        max_insert_time_slice: TimeDelta::hours(1),
        block_order: BlockOrder::EventTime,
        target_rows_per_query: 250_000,
    };

    // Run 1: only block 0 exists yet -- a single-block, degenerate [t0, t0] partition.
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
        1,
        "expected a single spec for block 0 alone"
    );
    let mut run1_same_run_ranges: Vec<TimeRange> = Vec::new();
    update_partition(
        lake.clone(),
        view_meta.clone(),
        schema.clone(),
        &convert_ticks,
        &run1_specs[0],
        &mut run1_same_run_ranges,
    )
    .await
    .with_context(|| "writing run 1 degenerate partition")?;

    // Push blocks 1 and 2 (deterministic event and insert order: t0+1s, t0+2s).
    push_and_insert_block(&ingestion, &mut stream, &process_info, "span_1").await?;
    push_and_insert_block(&ingestion, &mut stream, &process_info, "span_2").await?;
    for (object_offset, begin_ticks, end_ticks, insert_secs) in
        [(2i64, 1000i64, 2000i64, 1i64), (4, 2000, 3000, 2)]
    {
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
    regenerate_global_view(
        lakehouse.clone(),
        blocks_view.clone(),
        materialize_range,
        null_response_writer.clone(),
    )
    .await?;

    // Run 2 (a distinct jit_update loop, fresh same_run_ranges): regrouping now produces a
    // single, non-degenerate [t0, t0+2s] partition covering all 3 blocks.
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
        1,
        "expected a single spec covering all 3 blocks with a large max_nb_objects"
    );
    let mut run2_same_run_ranges: Vec<TimeRange> = Vec::new();
    update_partition(
        lake.clone(),
        view_meta,
        schema,
        &convert_ticks,
        &run2_specs[0],
        &mut run2_same_run_ranges,
    )
    .await
    .with_context(|| "writing run 2 growing partition")?;

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
        1,
        "run 2's growing partition must retire run 1's stale degenerate predecessor (found {} \
         partitions)",
        rows.len()
    );
    let b: DateTime<Utc> = rows[0].try_get("begin_insert_time")?;
    let e: DateTime<Utc> = rows[0].try_get("end_insert_time")?;
    assert_eq!(b, t0, "surviving partition should start at t0");
    assert!(
        e > b,
        "surviving partition should be run 2's non-degenerate [t0, t0+2s] partition, not the \
         stale degenerate one"
    );

    Ok(())
}

/// Same-run consecutive-degenerate-siblings case (review round 3, issue 3): several partitions
/// written in the *same* run can legally share an identical degenerate insert range (many blocks
/// registered at the exact same insert timestamp, each becoming its own single-block partition --
/// exercised at the grouping level by `jit_partition_grouping_tests.rs::degenerate_inputs`). Writing
/// the second (or third) must not retire its identically-ranged, just-written same-run
/// predecessor(s): unlike the shape-based exclusion this branch's predicate used to rely on, the
/// same_run_ranges exclusion protects a row because *this run* wrote it, regardless of how many
/// other rows share its exact range. Writes 3 single-block partitions, all with insert range
/// `[t, t]` (all 3 blocks share the same insert_time), in one simulated run, and asserts all 3
/// survive.
#[ignore]
#[tokio::test]
async fn thread_spans_same_run_consecutive_degenerate_siblings_survive() -> Result<()> {
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

    // Push 3 blocks with 1, 2, and 3 span pairs (2, 4, 6 objects) respectively: distinct object
    // counts, unlike `push_and_insert_block`'s uniform 2, are needed here so that
    // `is_jit_partition_up_to_date`'s object-count comparison (a coarse, count-only fingerprint --
    // `block_ids_hash` is just `nb_objects.to_le_bytes()`) cannot mistake one sibling's write
    // request for "already up to date" against a *different* sibling's just-committed,
    // identically-ranged row (same range, same count); that would wrongly skip the write this
    // test needs to reach `retire_partitions`.
    for (name, n_pairs) in [("span_0", 1usize), ("span_1", 2), ("span_2", 3)] {
        push_pairs_and_insert_block(&ingestion, &mut stream, &process_info, name, n_pairs).await?;
    }

    // Deterministic event order (matching object_offset order), but all 3 blocks share the exact
    // same insert_time `t0` -- the shape that lets `group_blocks_into_partitions` legally emit
    // several consecutive same-run partitions with an identical degenerate range once each is
    // sliced out on its own below. Truncated to microsecond precision (Postgres's timestamptz
    // columns only store microseconds), since the final assertions compare stored, read-back
    // boundaries against `t0` for exact equality.
    let t0 = (Utc::now() - TimeDelta::minutes(20)).duration_trunc(TimeDelta::microseconds(1))?;
    for (object_offset, begin_ticks, end_ticks) in
        [(0i64, 0i64, 1000i64), (2, 1000, 2000), (6, 2000, 3000)]
    {
        sqlx::query(
            "UPDATE blocks SET begin_ticks = $1, end_ticks = $2, insert_time = $3 \
             WHERE stream_id = $4 AND object_offset = $5",
        )
        .bind(begin_ticks)
        .bind(end_ticks)
        .bind(t0)
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

    let config = JitPartitionConfig {
        max_nb_objects: 1000,
        max_insert_time_slice: TimeDelta::hours(1),
        block_order: BlockOrder::EventTime,
        target_rows_per_query: 250_000,
    };
    let segment_partitions = PartitionCache::fetch_overlapping_insert_range_for_view(
        &lake.db_pool,
        blocks_view.get_view_set_name(),
        blocks_view.get_view_instance_id(),
        full_range,
    )
    .await?;
    let specs = generate_stream_jit_partitions_segment(
        &config,
        lakehouse.clone(),
        &blocks_view,
        &segment_partitions,
        &full_range,
        stream_meta.clone(),
        process_meta.clone(),
    )
    .await?;
    assert_eq!(
        specs.len(),
        1,
        "expected all 3 blocks in a single spec with a large max_nb_objects"
    );
    let all_blocks = &specs[0].blocks;
    assert_eq!(all_blocks.len(), 3, "expected all 3 blocks in the spec");

    // All 3 partitions are written by the same simulated jit_update run, sharing one
    // same_run_ranges accumulator, and all 3 have the identical degenerate range [t0, t0].
    let mut same_run_ranges: Vec<TimeRange> = Vec::new();
    for (i, block) in all_blocks.iter().enumerate() {
        let spec = slice_spec(std::slice::from_ref(block));
        update_partition(
            lake.clone(),
            view_meta.clone(),
            schema.clone(),
            &convert_ticks,
            &spec,
            &mut same_run_ranges,
        )
        .await
        .with_context(|| format!("writing same-run degenerate sibling {i}"))?;
    }

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
        "all 3 same-run degenerate siblings must survive (found {} partitions)",
        rows.len()
    );
    for row in &rows {
        let b: DateTime<Utc> = row.try_get("begin_insert_time")?;
        let e: DateTime<Utc> = row.try_get("end_insert_time")?;
        assert_eq!(b, t0, "every sibling should start at t0");
        assert_eq!(e, t0, "every sibling should be degenerate at t0");
    }

    Ok(())
}

/// Batched stream path equivalence (jit_batched_block_queries_plan.md, Testing Strategy): the
/// outer `generate_stream_jit_partitions` batches its block queries and then splits each batch
/// back into per-bucket runs (`BlockOrder::EventTime` uses this path for `net_spans` too, so this
/// is the riskiest new logic under that ordering). With `target_rows_per_query` lowered to force
/// more than one batch over a 4-bucket range (one block per bucket, so a batch of 2 covers 2
/// buckets), asserts the emitted specs are identical -- same block ids, in the same order, same
/// `block_ids_hash` -- to running `generate_stream_jit_partitions_segment` once per bucket
/// directly.
#[ignore]
#[tokio::test]
async fn thread_spans_batched_generation_matches_per_segment() -> Result<()> {
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

    // One block per hour bucket, 4 buckets, well in the past so this run's data cannot collide
    // with another concurrently-running test's (each test uses its own random process/stream id
    // regardless, but a round hour boundary keeps the math simple).
    let base_hour = (Utc::now() - TimeDelta::hours(10)).duration_trunc(TimeDelta::hours(1))?;
    for name in ["span_0", "span_1", "span_2", "span_3"] {
        push_and_insert_block(&ingestion, &mut stream, &process_info, name).await?;
    }
    for (i, (object_offset, begin_ticks, end_ticks)) in [
        (0i64, 0i64, 1000i64),
        (2, 1000, 2000),
        (4, 2000, 3000),
        (6, 3000, 4000),
    ]
    .into_iter()
    .enumerate()
    {
        let bucket_time = base_hour + TimeDelta::hours(i as i64) + TimeDelta::minutes(30);
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
        .with_context(|| format!("overriding block at object_offset {object_offset}"))?;
    }

    let lake = Arc::new(lake);
    let runtime = Arc::new(make_runtime_env()?);
    let lakehouse = Arc::new(LakehouseContext::new(lake.clone(), runtime.clone()));
    let view_factory = Arc::new(default_view_factory(runtime.clone(), lake.clone()).await?);
    let null_response_writer = Arc::new(ResponseWriter::new(None));

    let materialize_range = TimeRange::new(base_hour, base_hour + TimeDelta::hours(4));
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

    let full_range = TimeRange::new(
        base_hour - TimeDelta::seconds(10),
        base_hour + TimeDelta::hours(4) + TimeDelta::seconds(10),
    );

    let stream_meta = Arc::new(
        find_stream_from_view(lakehouse.clone(), view_factory.clone(), &stream_id, None).await?,
    );
    let (process_meta, _last_block_end_ticks, _last_block_end_time) =
        find_process_with_latest_timing(lakehouse.clone(), view_factory.clone(), &process_id, None)
            .await?;
    let process_meta = Arc::new(process_meta);

    // Force more than one batch (one block/bucket, target_rows_per_query=2 packs pairs of
    // adjacent buckets into one batch) while never forcing a cut *within* a bucket
    // (max_nb_objects large).
    let config = JitPartitionConfig {
        max_nb_objects: 1000,
        max_insert_time_slice: TimeDelta::hours(1),
        block_order: BlockOrder::EventTime,
        target_rows_per_query: 2,
    };

    let batched_specs = generate_stream_jit_partitions(
        &config,
        lakehouse.clone(),
        &blocks_view,
        &full_range,
        stream_meta.clone(),
        process_meta.clone(),
    )
    .await
    .with_context(|| "generate_stream_jit_partitions")?;

    // Expected: run the per-segment function once per hour bucket directly, and concatenate.
    let segment_source_partitions = PartitionCache::fetch_overlapping_insert_range_for_view(
        &lake.db_pool,
        blocks_view.get_view_set_name(),
        blocks_view.get_view_instance_id(),
        TimeRange::new(base_hour, base_hour + TimeDelta::hours(4)),
    )
    .await?;
    let mut expected_specs = vec![];
    for i in 0..4i64 {
        let bucket_range = TimeRange::new(
            base_hour + TimeDelta::hours(i),
            base_hour + TimeDelta::hours(i + 1),
        );
        let mut segment_specs = generate_stream_jit_partitions_segment(
            &config,
            lakehouse.clone(),
            &blocks_view,
            &segment_source_partitions,
            &bucket_range,
            stream_meta.clone(),
            process_meta.clone(),
        )
        .await
        .with_context(|| format!("generate_stream_jit_partitions_segment bucket {i}"))?;
        expected_specs.append(&mut segment_specs);
    }

    assert_eq!(
        batched_specs.len(),
        expected_specs.len(),
        "batched and per-segment generation must emit the same number of specs"
    );
    for (batched, expected) in batched_specs.iter().zip(expected_specs.iter()) {
        assert_eq!(
            batched.block_ids_hash, expected.block_ids_hash,
            "block_ids_hash must match"
        );
        let batched_ids: Vec<_> = batched.blocks.iter().map(|b| b.block.block_id).collect();
        let expected_ids: Vec<_> = expected.blocks.iter().map(|b| b.block.block_id).collect();
        assert_eq!(
            batched_ids, expected_ids,
            "block ids and order must match between the batched and per-segment paths"
        );
    }

    Ok(())
}
