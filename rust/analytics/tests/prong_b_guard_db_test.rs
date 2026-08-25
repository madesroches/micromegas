//! DB-backed tests for Query Enforcement Prong B (#1371, AbAC Stage 3): `AudienceGuard`'s
//! arg-addressed guards on `process_spans`, `perfetto_trace_chunks`, `parse_block`, `get_payload`,
//! and the row filter on `list_partitions`. Mirrors `ownership_rewrite_db_test.rs`'s convention
//! (seed through the real ingestion pipeline, `#[ignore]`, requires a live
//! `MICROMEGAS_SQL_CONNECTION_STRING`/`MICROMEGAS_OBJECT_STORE_URI`) and reuses its
//! `ProcessInfo.properties` stamping approach -- see that file's module doc comment for why
//! stamping must happen before any block is materialized.
//!
//! Unlike Prong A, Prong B resolves audiences straight from Postgres (`AudienceIndex`), so none
//! of these tests need the `processes`/`streams` global views materialized for the *guard* to
//! work -- only `blocks` (for `parse_block`/`get_payload`, neither of which touches `processes`)
//! and, for `process_spans`/`perfetto_trace_chunks`, whatever `get_process_thread_list`/
//! `get_process_exe` need to run without error, which is `blocks` and `processes` respectively.

mod common;

use anyhow::{Context, Result};
use chrono::{DurationRound, TimeDelta, Utc};
use common::db_fixtures::{
    caller_with_unstamped_audience, ensure_telemetry_guard, reset_global_view,
};
use micromegas_analytics::dfext::string_column_accessor::string_column_by_name;
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
    file: "prong_b_guard_db_test.rs",
    line: 1,
};

/// One seeded process with a "cpu" stream (one thread span pair, for `thread_spans`/
/// `process_spans`/`perfetto_trace_chunks`; one async span pair, for `async_events`) and its
/// resulting block id -- looked up from `blocks` after ingestion, since `StreamBlock::encode_bin`
/// generates the block id internally rather than returning it.
struct ProcessFixture {
    process_id: uuid::Uuid,
    cpu_stream_id: uuid::Uuid,
    cpu_block_id: uuid::Uuid,
}

/// Seeds one process -- stamped with `audience` via the real `insert_process(body,
/// &WriteAudience)` parameter (AbAC Stage 5, #1373) if `Some`, left unstamped if `None` -- plus
/// its cpu stream and one block, through the real ingestion pipeline. See
/// `ownership_rewrite_db_test.rs`'s identical-in-spirit helper for why stamping happens before
/// any block is materialized (irrelevant to Prong B, which reads Postgres directly, but kept for
/// consistency and because this file shares its seeding style).
async fn seed_process(
    ingestion: &WebIngestionService,
    pool: &sqlx::Pool<sqlx::Postgres>,
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

    let cpu_block_id: uuid::Uuid = sqlx::query_scalar(
        "SELECT block_id FROM blocks WHERE stream_id = $1 ORDER BY insert_time LIMIT 1",
    )
    .bind(cpu_stream_id)
    .fetch_one(pool)
    .await
    .with_context(|| "looking up the seeded cpu block's id")?;

    Ok(ProcessFixture {
        process_id,
        cpu_stream_id,
        cpu_block_id,
    })
}

fn caller(read_scope: ReadScope) -> CallerContext {
    CallerContext {
        read_scope,
        is_admin: false,
        isolation_config: Arc::new(IsolationConfig::default()),
        admin_principal_possible: true,
        identity: None,
        grant_selectors: Arc::from([]),
    }
}

fn scope(audiences: &[&str]) -> ReadScope {
    ReadScope::Audiences(
        audiences
            .iter()
            .map(|a| a.to_string())
            .collect::<Vec<_>>()
            .into(),
    )
}

/// Plans and executes `sql` under `caller`'s scope, returning the collected batches. A fresh
/// `SessionContext` per call, matching `ownership_rewrite_db_test.rs`'s `row_count` convention --
/// `AudienceGuard` is constructed once per `register_lakehouse_functions` call, from `caller`.
async fn query(
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    caller: CallerContext,
    query_range: Option<TimeRange>,
    sql: &str,
) -> Result<Vec<datafusion::arrow::array::RecordBatch>> {
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
    Ok(ctx.sql(sql).await?.collect().await?)
}

async fn row_count(
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    caller: CallerContext,
    query_range: Option<TimeRange>,
    sql: &str,
) -> Result<usize> {
    let batches = query(lakehouse, view_factory, caller, query_range, sql).await?;
    Ok(batches.iter().map(|b| b.num_rows()).sum())
}

/// Shared fixture setup: three processes (A/B stamped with different audiences, C unstamped),
/// with `blocks`/`processes`/`streams` materialized (needed for `process_spans`'
/// `get_process_thread_list` and `perfetto_trace_chunks`' `get_process_exe`, which read those
/// views under the witness's internal caller regardless of the outer caller's scope -- see
/// `tasks/1371_udtf_udf_guards_plan.md` §6).
struct Fixtures {
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    process_a: ProcessFixture,
    process_b: ProcessFixture,
    process_c: ProcessFixture,
    insert_range: TimeRange,
}

async fn setup() -> Result<Fixtures> {
    ensure_telemetry_guard();
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let lake = connect_to_data_lake(&connection_string, &object_store_uri).await?;
    let ingestion = WebIngestionService::new(lake.clone());
    let null_response_writer = Arc::new(ResponseWriter::new(None));

    let process_a = seed_process(&ingestion, &lake.db_pool, Some("team-a")).await?;
    let process_b = seed_process(&ingestion, &lake.db_pool, Some("team-b")).await?;
    let process_c = seed_process(&ingestion, &lake.db_pool, None).await?;

    let lake = Arc::new(lake);
    let runtime = Arc::new(micromegas_analytics::lakehouse::runtime::make_runtime_env()?);
    let lakehouse = Arc::new(LakehouseContext::new(lake.clone(), runtime.clone()));

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

    let view_factory = Arc::new(default_view_factory(runtime.clone(), lake.clone()).await?);

    Ok(Fixtures {
        lakehouse,
        view_factory,
        process_a,
        process_b,
        process_c,
        insert_range,
    })
}

#[ignore]
#[tokio::test]
async fn process_spans_guard_enforces_audience() -> Result<()> {
    let f = setup().await?;
    let own_sql = format!(
        "SELECT * FROM process_spans('{}', 'thread')",
        f.process_a.process_id
    );
    let own_rows = row_count(
        f.lakehouse.clone(),
        f.view_factory.clone(),
        caller(scope(&["team-a"])),
        Some(f.insert_range),
        &own_sql,
    )
    .await?;
    assert!(
        own_rows > 0,
        "the owning audience must see its own process_spans"
    );

    let all_rows = row_count(
        f.lakehouse.clone(),
        f.view_factory.clone(),
        caller(ReadScope::All),
        Some(f.insert_range),
        &own_sql,
    )
    .await?;
    assert_eq!(
        all_rows, own_rows,
        "ReadScope::All must see the same rows as the owning audience (no over-blocking)"
    );

    let foreign_sql = format!(
        "SELECT * FROM process_spans('{}', 'thread')",
        f.process_b.process_id
    );
    let err = query(
        f.lakehouse.clone(),
        f.view_factory.clone(),
        caller(scope(&["team-a"])),
        Some(f.insert_range),
        &foreign_sql,
    )
    .await
    .expect_err("a foreign audience's process must be denied");
    let foreign_msg = err.to_string();
    assert!(
        foreign_msg.contains("not found or not accessible"),
        "expected the uniform not-found-shaped denial, got: {foreign_msg}"
    );

    let random_id = uuid::Uuid::new_v4();
    let random_sql = format!("SELECT * FROM process_spans('{random_id}', 'thread')");
    let random_err = query(
        f.lakehouse.clone(),
        f.view_factory.clone(),
        caller(scope(&["team-a"])),
        Some(f.insert_range),
        &random_sql,
    )
    .await
    .expect_err("a nonexistent process must be denied the same way");
    let foreign_msg_shape = foreign_msg.replace(&f.process_b.process_id.to_string(), "ID");
    let random_msg_shape = random_err.to_string().replace(&random_id.to_string(), "ID");
    assert_eq!(
        foreign_msg_shape, random_msg_shape,
        "denial for a foreign process and denial for a nonexistent one must be shaped the same, \
         so a caller can't tell them apart (no existence oracle)"
    );

    let unstamped_sql = format!(
        "SELECT * FROM process_spans('{}', 'thread')",
        f.process_c.process_id
    );
    query(
        f.lakehouse.clone(),
        f.view_factory.clone(),
        caller(scope(&["team-a"])),
        Some(f.insert_range),
        &unstamped_sql,
    )
    .await
    .expect_err(
        "an unstamped process must stay denied for a caller whose scope doesn't include the \
         default MICROMEGAS_UNSTAMPED_AUDIENCE value ('public')",
    );
    let unstamped_rows = row_count(
        f.lakehouse.clone(),
        f.view_factory.clone(),
        caller_with_unstamped_audience(scope(&["team-a"]), "team-a"),
        Some(f.insert_range),
        &unstamped_sql,
    )
    .await?;
    assert!(
        unstamped_rows > 0,
        "an unstamped process must become readable once MICROMEGAS_UNSTAMPED_AUDIENCE names an \
         audience in the caller's own scope"
    );

    Ok(())
}

#[ignore]
#[tokio::test]
async fn perfetto_trace_chunks_guard_enforces_audience() -> Result<()> {
    let f = setup().await?;
    let time_range = format!(
        "TIMESTAMP '{}', TIMESTAMP '{}'",
        f.insert_range.begin.to_rfc3339(),
        f.insert_range.end.to_rfc3339()
    );
    let own_sql = format!(
        "SELECT * FROM perfetto_trace_chunks('{}', 'both', {})",
        f.process_a.process_id, time_range
    );
    let own_rows = row_count(
        f.lakehouse.clone(),
        f.view_factory.clone(),
        caller(scope(&["team-a"])),
        None,
        &own_sql,
    )
    .await?;
    assert!(
        own_rows > 0,
        "the owning audience must see its own perfetto trace chunks"
    );

    let foreign_sql = format!(
        "SELECT * FROM perfetto_trace_chunks('{}', 'both', {})",
        f.process_b.process_id, time_range
    );
    let err = query(
        f.lakehouse.clone(),
        f.view_factory.clone(),
        caller(scope(&["team-a"])),
        None,
        &foreign_sql,
    )
    .await
    .expect_err("a foreign audience's process must be denied");
    assert!(
        err.to_string().contains("not found or not accessible"),
        "expected the uniform not-found-shaped denial, got: {err}"
    );

    Ok(())
}

#[ignore]
#[tokio::test]
async fn parse_block_guard_enforces_audience_and_skips_processes_dependency() -> Result<()> {
    let f = setup().await?;
    let own_sql = format!("SELECT * FROM parse_block('{}')", f.process_a.cpu_block_id);
    let own_rows = row_count(
        f.lakehouse.clone(),
        f.view_factory.clone(),
        caller(scope(&["team-a"])),
        None,
        &own_sql,
    )
    .await?;
    assert!(
        own_rows > 0,
        "the owning audience must see its own block's parsed objects"
    );

    let foreign_sql = format!("SELECT * FROM parse_block('{}')", f.process_b.cpu_block_id);
    let err = query(
        f.lakehouse.clone(),
        f.view_factory.clone(),
        caller(scope(&["team-a"])),
        None,
        &foreign_sql,
    )
    .await
    .expect_err("a foreign audience's block must be denied");
    assert!(
        err.to_string().contains("not found or not accessible"),
        "expected the uniform not-found-shaped denial, got: {err}"
    );

    // §6 regression window: `parse_block` never touches `processes`, only `blocks` (already
    // materialized by `setup()`) -- unlike `process_spans`/`perfetto_trace_chunks`, whose inner
    // `get_process_thread_list`/`get_process_exe` calls do, so those two are excluded from this
    // assertion (they already fail in this window under `ReadScope::All` today, guard or no
    // guard -- a pre-existing daemon-materialization gap, out of scope here).
    let own_rows_again = row_count(
        f.lakehouse.clone(),
        f.view_factory.clone(),
        caller(scope(&["team-a"])),
        None,
        &own_sql,
    )
    .await?;
    assert_eq!(own_rows_again, own_rows);

    Ok(())
}

#[ignore]
#[tokio::test]
async fn get_payload_guard_enforces_audience_and_denies_mixed_batches() -> Result<()> {
    let f = setup().await?;
    let own_sql = format!(
        "SELECT get_payload('{}', '{}', '{}') AS payload",
        f.process_a.process_id, f.process_a.cpu_stream_id, f.process_a.cpu_block_id
    );
    let own_batches = query(
        f.lakehouse.clone(),
        f.view_factory.clone(),
        caller(scope(&["team-a"])),
        None,
        &own_sql,
    )
    .await?;
    let own_bytes: &datafusion::arrow::array::BinaryArray = own_batches[0]
        .column_by_name("payload")
        .expect("payload column")
        .as_any()
        .downcast_ref()
        .expect("payload is Binary");
    assert!(
        !own_bytes.value(0).is_empty(),
        "the owning audience must get its own payload back"
    );

    let foreign_sql = format!(
        "SELECT get_payload('{}', '{}', '{}') AS payload",
        f.process_b.process_id, f.process_b.cpu_stream_id, f.process_b.cpu_block_id
    );
    let err = query(
        f.lakehouse.clone(),
        f.view_factory.clone(),
        caller(scope(&["team-a"])),
        None,
        &foreign_sql,
    )
    .await
    .expect_err("a foreign audience's payload must be denied");
    assert!(
        err.to_string().contains("not found or not accessible"),
        "expected the uniform not-found-shaped denial, got: {err}"
    );

    let mixed_sql = format!(
        "SELECT get_payload(process_id, stream_id, block_id) AS payload FROM (VALUES \
         ('{}', '{}', '{}'), ('{}', '{}', '{}')) AS t(process_id, stream_id, block_id)",
        f.process_a.process_id,
        f.process_a.cpu_stream_id,
        f.process_a.cpu_block_id,
        f.process_b.process_id,
        f.process_b.cpu_stream_id,
        f.process_b.cpu_block_id,
    );
    query(
        f.lakehouse.clone(),
        f.view_factory.clone(),
        caller(scope(&["team-a"])),
        None,
        &mixed_sql,
    )
    .await
    .expect_err(
        "a batch mixing a readable and an unreadable process id must fail the whole call, \
         never return a partial/NULL result",
    );

    Ok(())
}

#[ignore]
#[tokio::test]
async fn list_partitions_row_filter_enforces_audience() -> Result<()> {
    let f = setup().await?;

    // Force JIT materialization of each process's `thread_spans` instance partition (a
    // stream-scoped view set, so `list_partitions`' `IdKind::ProcessOrStream` resolution matches
    // it via the 'stream' arm) so `lakehouse_partitions` has rows to filter -- also exercises the
    // `stream_id`-resolution path the guard needs for `thread_spans`, the one view set with no
    // process-scoped alternative.
    for stream_id in [f.process_a.cpu_stream_id, f.process_b.cpu_stream_id] {
        let sql = format!("SELECT * FROM view_instance('thread_spans', '{stream_id}')");
        row_count(
            f.lakehouse.clone(),
            f.view_factory.clone(),
            caller(ReadScope::All),
            Some(f.insert_range),
            &sql,
        )
        .await?;
    }

    let all_batches = query(
        f.lakehouse.clone(),
        f.view_factory.clone(),
        caller(ReadScope::All),
        None,
        "SELECT view_set_name, view_instance_id FROM list_partitions()",
    )
    .await?;
    let all_rows: usize = all_batches.iter().map(|b| b.num_rows()).sum();
    assert!(
        all_rows > 0,
        "ReadScope::All must see at least the partitions just materialized"
    );

    let team_a_batches = query(
        f.lakehouse.clone(),
        f.view_factory.clone(),
        caller(scope(&["team-a"])),
        None,
        "SELECT view_set_name, view_instance_id FROM list_partitions()",
    )
    .await?;
    let team_a_instance_ids: Vec<String> = team_a_batches
        .iter()
        .flat_map(|b| {
            let ids = string_column_by_name(b, "view_instance_id").expect("view_instance_id");
            (0..b.num_rows())
                .map(move |i| ids.value(i).expect("valid utf8").to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        team_a_instance_ids.contains(&f.process_a.cpu_stream_id.to_string()),
        "team-a must see its own thread_spans instance row in list_partitions"
    );
    assert!(
        !team_a_instance_ids.contains(&f.process_b.cpu_stream_id.to_string()),
        "team-a must not see team-b's thread_spans instance row in list_partitions"
    );
    assert!(
        !team_a_instance_ids.iter().any(|id| id == "global"),
        "'global' rows must stay hidden from a caller whose scope doesn't include the default \
         MICROMEGAS_UNSTAMPED_AUDIENCE value ('public')"
    );

    let team_a_with_public_batches = query(
        f.lakehouse.clone(),
        f.view_factory.clone(),
        caller_with_unstamped_audience(scope(&["team-a"]), "team-a"),
        None,
        "SELECT view_set_name, view_instance_id FROM list_partitions()",
    )
    .await?;
    let has_global = team_a_with_public_batches.iter().any(|b| {
        let ids = string_column_by_name(b, "view_instance_id").expect("view_instance_id");
        (0..b.num_rows()).any(|i| ids.value(i).expect("valid utf8") == "global")
    });
    assert!(
        has_global,
        "'global' rows must become visible once MICROMEGAS_UNSTAMPED_AUDIENCE names an \
         audience in the caller's own scope"
    );

    // `LIMIT n` over a filtered set must never return fewer than `min(n, matching)` rows because
    // of a pushed-down LIMIT racing the filter.
    let limited_batches = query(
        f.lakehouse.clone(),
        f.view_factory.clone(),
        caller(scope(&["team-a"])),
        None,
        "SELECT view_set_name, view_instance_id FROM list_partitions() LIMIT 1",
    )
    .await?;
    let limited_rows: usize = limited_batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(
        limited_rows,
        1.min(team_a_instance_ids.len()),
        "LIMIT 1 over team-a's filtered set must return exactly min(1, matching) rows"
    );

    Ok(())
}
