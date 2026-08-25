//! DB-backed tests for `OwnershipRewrite` (#1370, AbAC Stage 2) -- the issue's own acceptance
//! criteria: seed processes stamped with different `micromegas.audience` properties (plus one
//! never stamped at all) through the real ingestion pipeline, materialize the `blocks`/
//! `processes`/`streams` batch views `OwnershipRewrite` reads its audience mapping from, then
//! assert a session's visible rows differ by `CallerContext.read_scope` -- cross-audience denial,
//! same-audience visibility, `ReadScope::All` sees everything, the
//! `MICROMEGAS_UNSTAMPED_AUDIENCE` escape hatch, and (the coverage a naive "process_id column or
//! bust" implementation would miss) the two schema-less view sets `async_events`/`thread_spans`.
//!
//! Requires a live `MICROMEGAS_SQL_CONNECTION_STRING` / `MICROMEGAS_OBJECT_STORE_URI` (mirrors
//! `net_spans_retire_overlap_db_test.rs`'s / `thread_spans_ordering_db_test.rs`'s convention);
//! does not run under a plain `cargo test`.
//!
//! `micromegas.audience` ingestion-time stamping now exists (Stage 5, #1373): this file stamps
//! through the real `insert_process(body, &WriteAudience)` parameter, exactly the path a real
//! ingestion key exercises, rather than hand-writing the property on the `ProcessInfo` passed in
//! (which would now be stripped anyway -- `finalize_process_properties` drops any
//! client-supplied `micromegas.*` key). Critically, stamping must happen *before* the `blocks`
//! view's partitions are materialized, not merely before the `processes` view's own
//! materialization: `BlocksView::data_sql` snapshots `processes.properties` from Postgres into
//! the `blocks` parquet partitions at materialization time (`blocks_view.rs`), and the
//! `processes` `SqlBatchView`'s transform query reads `first_value("processes.properties") ...
//! FROM blocks` -- i.e. from the already-materialized `blocks` partitions, never from Postgres
//! directly (`processes_view.rs`). Stamping the process at creation time (before any block
//! exists) trivially satisfies this ordering.

mod common;

use anyhow::{Context, Result};
use chrono::{DurationRound, TimeDelta, Utc};
use common::db_fixtures::{
    caller_with_unstamped_audience, ensure_telemetry_guard, reset_global_view,
};
use micromegas_analytics::lakehouse::blocks_view::BlocksView;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::LivePartitionProvider;
use micromegas_analytics::lakehouse::processes_view::make_processes_view;
use micromegas_analytics::lakehouse::query::make_session_context;
use micromegas_analytics::lakehouse::read_scope::{CallerContext, IsolationConfig, ReadScope};
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::streams_view::make_streams_view;
use micromegas_analytics::lakehouse::view_factory::{ViewFactory, default_view_factory};
use micromegas_analytics::response_writer::ResponseWriter;
use micromegas_analytics::time::TimeRange;
use micromegas_ingestion::data_lake_connection::connect_to_data_lake;
use micromegas_ingestion::web_ingestion_service::WebIngestionService;
use micromegas_ingestion::write_audience::WriteAudience;
use micromegas_telemetry::wire_format::encode_cbor;
use micromegas_telemetry_sink::stream_block::StreamBlock;
use micromegas_telemetry_sink::stream_info::make_stream_info;
use micromegas_tracing::dispatch::make_process_info;
use micromegas_tracing::event::TracingBlock;
use micromegas_tracing::logs::{LogBlock, LogStaticStrInteropEvent, LogStream};
use micromegas_tracing::spans::{
    BeginAsyncNamedSpanEvent, BeginThreadNamedSpanEvent, EndAsyncNamedSpanEvent,
    EndThreadNamedSpanEvent, SpanLocation, ThreadBlock, ThreadStream,
};
use micromegas_tracing::time::now;
use std::collections::HashMap;
use std::sync::Arc;

static SPAN_LOCATION: SpanLocation = SpanLocation {
    lod: micromegas_tracing::levels::Verbosity::Med,
    target: "target",
    module_path: "module_path",
    file: "ownership_rewrite_db_test.rs",
    line: 1,
};

/// One seeded process, its "cpu" stream (carrying one thread span pair, for `thread_spans`, and
/// one async span pair, for `async_events`) and its "log" stream (carrying one log entry, for
/// `log_entries`).
struct ProcessFixture {
    process_id: uuid::Uuid,
    cpu_stream_id: uuid::Uuid,
}

/// Seeds one process -- stamped with `audience` via the real `insert_process(body,
/// &WriteAudience)` parameter (AbAC Stage 5, #1373) if `Some`, left unstamped if `None` -- plus
/// its cpu/log streams and one block each, through the real ingestion pipeline
/// (`WebIngestionService`, the same entry point a real client hits).
async fn seed_process(
    ingestion: &WebIngestionService,
    audience: Option<&str>,
) -> Result<ProcessFixture> {
    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, None, HashMap::new());
    let process_body = bytes::Bytes::from(encode_cbor(&process_info)?);
    let write_audience = WriteAudience::new(audience)?;
    ingestion
        .insert_process(process_body, &write_audience)
        .await
        .map_err(|e| anyhow::anyhow!("insert_process: {e}"))?;

    // "cpu" stream: one thread span pair (thread_spans) and one async span pair (async_events),
    // in the same block -- async span events ride the thread event queue (see
    // `dispatch.rs::on_begin_async_named_scope`, which calls `on_thread_event`).
    let mut cpu_stream = ThreadStream::new(1024, process_id, &["cpu".to_owned()], HashMap::new());
    let cpu_stream_id = cpu_stream.stream_id();
    let cpu_stream_info = make_stream_info(&cpu_stream);
    ingestion
        .insert_stream(bytes::Bytes::from(encode_cbor(&cpu_stream_info)?))
        .await
        .map_err(|e| anyhow::anyhow!("insert_stream (cpu): {e}"))?;
    let t0 = now();
    cpu_stream.get_events_mut().push(BeginThreadNamedSpanEvent {
        thread_span_location: &SPAN_LOCATION,
        name: "span".into(),
        time: t0,
    });
    cpu_stream.get_events_mut().push(EndThreadNamedSpanEvent {
        thread_span_location: &SPAN_LOCATION,
        name: "span".into(),
        time: t0 + 1_000_000,
    });
    cpu_stream.get_events_mut().push(BeginAsyncNamedSpanEvent {
        span_location: &SPAN_LOCATION,
        name: "async_span".into(),
        span_id: 1,
        parent_span_id: 0,
        depth: 0,
        time: t0,
    });
    cpu_stream.get_events_mut().push(EndAsyncNamedSpanEvent {
        span_location: &SPAN_LOCATION,
        name: "async_span".into(),
        span_id: 1,
        parent_span_id: 0,
        depth: 0,
        time: t0 + 1_000_000,
    });
    let cpu_next_offset =
        cpu_stream.get_block_ref().object_offset() + cpu_stream.get_block_ref().nb_objects();
    let mut cpu_block = cpu_stream.replace_block(Arc::new(ThreadBlock::new(
        1024,
        process_id,
        cpu_stream_id,
        cpu_next_offset,
    )));
    Arc::get_mut(&mut cpu_block)
        .context("sole owner of freshly replaced cpu block")?
        .close();
    let cpu_encoded = cpu_block.encode_bin(&process_info)?;
    ingestion
        .insert_block(bytes::Bytes::from(cpu_encoded))
        .await
        .map_err(|e| anyhow::anyhow!("insert_block (cpu): {e}"))?;

    // "log" stream: one log entry (log_entries).
    let mut log_stream = LogStream::new(1024, process_id, &["log".to_owned()], HashMap::new());
    let log_stream_id = log_stream.stream_id();
    let log_stream_info = make_stream_info(&log_stream);
    ingestion
        .insert_stream(bytes::Bytes::from(encode_cbor(&log_stream_info)?))
        .await
        .map_err(|e| anyhow::anyhow!("insert_stream (log): {e}"))?;
    log_stream.get_events_mut().push(LogStaticStrInteropEvent {
        time: now(),
        level: 4,
        target: "target".into(),
        msg: "hello".into(),
    });
    let log_next_offset =
        log_stream.get_block_ref().object_offset() + log_stream.get_block_ref().nb_objects();
    let mut log_block = log_stream.replace_block(Arc::new(LogBlock::new(
        1024,
        process_id,
        log_stream_id,
        log_next_offset,
    )));
    Arc::get_mut(&mut log_block)
        .context("sole owner of freshly replaced log block")?
        .close();
    let log_encoded = log_block.encode_bin(&process_info)?;
    ingestion
        .insert_block(bytes::Bytes::from(log_encoded))
        .await
        .map_err(|e| anyhow::anyhow!("insert_block (log): {e}"))?;

    Ok(ProcessFixture {
        process_id,
        cpu_stream_id,
    })
}

/// Plans and executes `sql` under `caller`'s scope, returning the total row count across every
/// returned batch. A fresh `SessionContext` per call, matching how a real request-scoped session
/// works -- `OwnershipRewrite` is constructed once per `make_session_context` call.
///
/// `query_range` is threaded straight through to `make_session_context`: most view sets don't
/// need one (`None` is correct there), but `ThreadSpansView::jit_update` hard-requires a bounded
/// range to know what to materialize on demand (`thread_spans_view.rs`) -- callers hitting
/// `thread_spans` must pass `Some(...)`, mirroring `thread_spans_ordering_db_test.rs`.
async fn row_count(
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    caller: CallerContext,
    query_range: Option<TimeRange>,
    sql: &str,
) -> Result<usize> {
    let part_provider = Arc::new(LivePartitionProvider::new(lakehouse.lake().db_pool.clone()));
    let ctx = make_session_context(
        lakehouse,
        part_provider,
        query_range,
        view_factory,
        Arc::new(NoOpSessionConfigurator),
        caller,
    )
    .await
    .with_context(|| "make_session_context")?;
    let batches = ctx.sql(sql).await?.collect().await?;
    Ok(batches.iter().map(|b| b.num_rows()).sum())
}

fn audiences_scope(audiences: &[&str]) -> ReadScope {
    ReadScope::Audiences(
        audiences
            .iter()
            .map(|a| a.to_string())
            .collect::<Vec<_>>()
            .into(),
    )
}

fn caller_with_scope(read_scope: ReadScope) -> CallerContext {
    CallerContext {
        read_scope,
        is_admin: false,
        isolation_config: Arc::new(IsolationConfig::default()),
        admin_principal_possible: true,
        identity: None,
        grant_selectors: Arc::from([]),
    }
}

#[ignore]
#[tokio::test]
async fn ownership_rewrite_enforces_audience_visibility() -> Result<()> {
    ensure_telemetry_guard();
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let lake = connect_to_data_lake(&connection_string, &object_store_uri).await?;
    let ingestion = WebIngestionService::new(lake.clone());
    let null_response_writer = Arc::new(ResponseWriter::new(None));

    // Seed three processes *before* any block/view materialization: A and B are stamped with
    // different audiences, C is left unstamped (no `micromegas.audience` property at all) --
    // exercising the `MICROMEGAS_UNSTAMPED_AUDIENCE` escape hatch below.
    let process_a = seed_process(&ingestion, Some("team-a")).await?;
    let process_b = seed_process(&ingestion, Some("team-b")).await?;
    let process_c = seed_process(&ingestion, None).await?;

    let lake = Arc::new(lake);
    let runtime = Arc::new(micromegas_analytics::lakehouse::runtime::make_runtime_env()?);
    let lakehouse = Arc::new(LakehouseContext::new(lake.clone(), runtime.clone()));

    // Materialize `blocks` (snapshots `processes.properties` from Postgres into the blocks
    // partitions), then `processes`/`streams` (the `SqlBatchView`s `OwnershipRewrite` reads its
    // audience mapping from -- their `jit_update` is a no-op, so nothing materializes them on
    // demand at query time the way the per-process JIT views below are).
    let insert_begin = (Utc::now() - TimeDelta::hours(1)).duration_trunc(TimeDelta::hours(1))?;
    let insert_range = TimeRange::new(insert_begin, insert_begin + TimeDelta::hours(3));
    let blocks_view = Arc::new(BlocksView::new()?);
    reset_global_view(
        lakehouse.clone(),
        blocks_view.clone(),
        insert_range,
        null_response_writer.clone(),
    )
    .await?;
    let blocks_only_factory = Arc::new(ViewFactory::new(vec![blocks_view.clone()]));
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

    // The full default factory: `processes`/`streams` (just materialized above) plus every
    // per-process/per-stream JIT view set (`log_entries`, `async_events`, `thread_spans`, ...).
    let view_factory = Arc::new(default_view_factory(runtime.clone(), lake.clone()).await?);

    // --- `processes`, directly -----------------------------------------------------------
    let processes_a_sql = format!(
        "SELECT * FROM processes WHERE process_id = '{}'",
        process_a.process_id
    );
    let processes_b_sql = format!(
        "SELECT * FROM processes WHERE process_id = '{}'",
        process_b.process_id
    );

    assert_eq!(
        row_count(
            lakehouse.clone(),
            view_factory.clone(),
            caller_with_scope(audiences_scope(&["team-a"])),
            None,
            &processes_a_sql,
        )
        .await?,
        1,
        "a caller scoped to user:a must see process A directly via `processes`"
    );
    assert_eq!(
        row_count(
            lakehouse.clone(),
            view_factory.clone(),
            caller_with_scope(audiences_scope(&["team-b"])),
            None,
            &processes_a_sql,
        )
        .await?,
        0,
        "a caller scoped to user:b must not see process A via `processes`"
    );
    assert_eq!(
        row_count(
            lakehouse.clone(),
            view_factory.clone(),
            caller_with_scope(audiences_scope(&["team-a"])),
            None,
            &processes_b_sql,
        )
        .await?,
        0,
        "a caller scoped to user:a must not see process B via `processes`"
    );
    assert_eq!(
        row_count(
            lakehouse.clone(),
            view_factory.clone(),
            caller_with_scope(audiences_scope(&["team-other"])),
            None,
            &processes_a_sql,
        )
        .await?,
        0,
        "a caller whose scope contains neither audience must see nothing"
    );
    assert_eq!(
        row_count(
            lakehouse.clone(),
            view_factory.clone(),
            caller_with_scope(ReadScope::All),
            None,
            &processes_a_sql,
        )
        .await?,
        1,
        "ReadScope::All (CallerContext::maintenance()) must see process A regardless of audience"
    );
    assert_eq!(
        row_count(
            lakehouse.clone(),
            view_factory.clone(),
            caller_with_scope(ReadScope::All),
            None,
            &processes_b_sql,
        )
        .await?,
        1,
        "ReadScope::All must see process B regardless of audience"
    );

    // --- `log_entries`, a process_id-**column** view, via `view_instance` ----------------
    let log_entries_a_sql = format!(
        "SELECT * FROM view_instance('log_entries', '{}')",
        process_a.process_id
    );
    assert_eq!(
        row_count(
            lakehouse.clone(),
            view_factory.clone(),
            caller_with_scope(audiences_scope(&["team-a"])),
            None,
            &log_entries_a_sql,
        )
        .await?,
        1,
        "a caller scoped to user:a must see process A's log_entries"
    );
    assert_eq!(
        row_count(
            lakehouse.clone(),
            view_factory.clone(),
            caller_with_scope(audiences_scope(&["team-b"])),
            None,
            &log_entries_a_sql,
        )
        .await?,
        0,
        "a caller scoped to user:b must not see process A's log_entries"
    );
    assert_eq!(
        row_count(
            lakehouse.clone(),
            view_factory.clone(),
            caller_with_scope(ReadScope::All),
            None,
            &log_entries_a_sql,
        )
        .await?,
        1,
        "ReadScope::All must see process A's log_entries"
    );

    // --- `async_events`, process-scoped with **no** process_id column (§5) ---------------
    let async_events_a_sql = format!(
        "SELECT * FROM view_instance('async_events', '{}')",
        process_a.process_id
    );
    let async_events_a_own = row_count(
        lakehouse.clone(),
        view_factory.clone(),
        caller_with_scope(audiences_scope(&["team-a"])),
        None,
        &async_events_a_sql,
    )
    .await?;
    assert!(
        async_events_a_own > 0,
        "a caller scoped to user:a must see process A's async_events (begin+end of one span)"
    );
    assert_eq!(
        row_count(
            lakehouse.clone(),
            view_factory.clone(),
            caller_with_scope(audiences_scope(&["team-b"])),
            None,
            &async_events_a_sql,
        )
        .await?,
        0,
        "a caller scoped to user:b must not see process A's async_events -- the naive \
         'process_id column or bust' implementation this test guards against would leave this \
         view set unfiltered entirely"
    );
    assert_eq!(
        row_count(
            lakehouse.clone(),
            view_factory.clone(),
            caller_with_scope(ReadScope::All),
            None,
            &async_events_a_sql,
        )
        .await?,
        async_events_a_own,
        "ReadScope::All must see the same async_events rows as the owning audience"
    );

    // --- `thread_spans`, stream-scoped with **no** process_id or stream_id column (§6) ---
    let thread_spans_a_sql = format!(
        "SELECT * FROM view_instance('thread_spans', '{}')",
        process_a.cpu_stream_id
    );
    let thread_spans_a_own = row_count(
        lakehouse.clone(),
        view_factory.clone(),
        caller_with_scope(audiences_scope(&["team-a"])),
        // `ThreadSpansView::jit_update` hard-requires a bounded range (thread_spans_view.rs);
        // `insert_range` already covers the real-clock `now()` these spans were stamped with
        // (see `seed_process`), so it doubles as the query range here without inventing a new
        // one.
        Some(insert_range),
        &thread_spans_a_sql,
    )
    .await?;
    assert!(
        thread_spans_a_own > 0,
        "a caller scoped to user:a must see process A's thread_spans"
    );
    assert_eq!(
        row_count(
            lakehouse.clone(),
            view_factory.clone(),
            caller_with_scope(audiences_scope(&["team-b"])),
            Some(insert_range),
            &thread_spans_a_sql,
        )
        .await?,
        0,
        "a caller scoped to user:b must not see process A's thread_spans"
    );
    assert_eq!(
        row_count(
            lakehouse.clone(),
            view_factory.clone(),
            caller_with_scope(ReadScope::All),
            Some(insert_range),
            &thread_spans_a_sql,
        )
        .await?,
        thread_spans_a_own,
        "ReadScope::All must see the same thread_spans rows as the owning audience"
    );

    // --- Unstamped process C: visible only via the MICROMEGAS_UNSTAMPED_AUDIENCE escape hatch
    let processes_c_sql = format!(
        "SELECT * FROM processes WHERE process_id = '{}'",
        process_c.process_id
    );
    assert_eq!(
        row_count(
            lakehouse.clone(),
            view_factory.clone(),
            caller_with_scope(audiences_scope(&["team-a"])),
            None,
            &processes_c_sql,
        )
        .await?,
        0,
        "an unstamped process must stay invisible to a ReadScope::Audiences caller whose own \
         scope doesn't include the default MICROMEGAS_UNSTAMPED_AUDIENCE value ('public'), \
         however unrelated to A/B its own scope is"
    );
    assert_eq!(
        row_count(
            lakehouse.clone(),
            view_factory.clone(),
            caller_with_unstamped_audience(audiences_scope(&["everyone"]), "everyone",),
            None,
            &processes_c_sql,
        )
        .await?,
        1,
        "a caller whose scope includes the configured MICROMEGAS_UNSTAMPED_AUDIENCE value must \
         see the unstamped process"
    );
    assert_eq!(
        row_count(
            lakehouse.clone(),
            view_factory.clone(),
            caller_with_unstamped_audience(audiences_scope(&["team-a"]), "everyone"),
            None,
            &processes_c_sql,
        )
        .await?,
        0,
        "configuring the escape hatch must not leak the unstamped process to a caller whose own \
         scope does not include the configured unstamped audience"
    );

    Ok(())
}
