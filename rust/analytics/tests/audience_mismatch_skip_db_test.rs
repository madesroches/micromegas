//! DB-backed regression test for the audience-mismatch attack: a block written under audience
//! `alpha`, carrying a `process_id` owned by `beta`, is excluded from `blocks`' own
//! materialization by the audience-mismatch predicate in `blocks_view.rs`, and therefore also
//! absent from every downstream view (`log_entries`, `measures`, `log_stats`), while the
//! victim's own correctly-stamped rows still materialize normally.
//!
//! Mirrors `ownership_rewrite_db_test.rs`'s harness (seeds through the real ingestion pipeline,
//! `#[ignore]`, requires a live `MICROMEGAS_SQL_CONNECTION_STRING`/`MICROMEGAS_OBJECT_STORE_URI`).
//! Assertions are on observable materialized state only, never on the `warn!`/`imetric!` side
//! effects `MetadataPartitionSpec::write` emits.

mod common;

use anyhow::{Context, Result};
use chrono::{DurationRound, TimeDelta, Utc};
use common::db_fixtures::{ensure_telemetry_guard, reset_global_view};
use micromegas_analytics::dfext::string_column_accessor::string_column_by_name;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::LivePartitionProvider;
use micromegas_analytics::lakehouse::query::query;
use micromegas_analytics::lakehouse::read_scope::CallerContext;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::view_factory::default_view_factory;
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
use micromegas_tracing::levels::Verbosity;
use micromegas_tracing::logs::{LogBlock, LogStaticStrInteropEvent, LogStream};
use micromegas_tracing::metrics::{
    IntegerMetricEvent, MetricsBlock, MetricsStream, StaticMetricMetadata,
};
use micromegas_tracing::process_info::ProcessInfo;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

static VICTIM_METRIC_DESC: StaticMetricMetadata = StaticMetricMetadata {
    lod: Verbosity::Med,
    name: "victim-metric",
    unit: "count",
    target: "target",
    file: "audience_mismatch_skip_db_test.rs",
    line: 1,
};

static ATTACKER_METRIC_DESC: StaticMetricMetadata = StaticMetricMetadata {
    lod: Verbosity::Med,
    name: "attacker-metric",
    unit: "count",
    target: "target",
    file: "audience_mismatch_skip_db_test.rs",
    line: 2,
};

/// Pushes one log event, closes the current block, and returns its CBOR-encoded wire body --
/// ready for `insert_block`.
fn make_log_block_body(
    stream: &mut LogStream,
    process_info: &ProcessInfo,
    msg: &'static str,
) -> Result<bytes::Bytes> {
    stream.get_events_mut().push(LogStaticStrInteropEvent {
        time: micromegas_tracing::time::now(),
        level: 4,
        target: "target".into(),
        msg: msg.into(),
    });
    let next_offset = stream.get_block_ref().object_offset() + stream.get_block_ref().nb_objects();
    let mut block = stream.replace_block(Arc::new(LogBlock::new(
        1024,
        stream.process_id(),
        stream.stream_id(),
        next_offset,
    )));
    Arc::get_mut(&mut block)
        .with_context(|| "sole owner of freshly replaced log block")?
        .close();
    Ok(bytes::Bytes::from(block.encode_bin(process_info)?))
}

/// Pushes one integer metric event, closes the current block, and returns its CBOR-encoded wire
/// body -- ready for `insert_block`.
fn make_metric_block_body(
    stream: &mut MetricsStream,
    process_info: &ProcessInfo,
    desc: &'static StaticMetricMetadata,
    value: u64,
) -> Result<bytes::Bytes> {
    stream.get_events_mut().push(IntegerMetricEvent {
        desc,
        value,
        time: micromegas_tracing::time::now(),
    });
    let next_offset = stream.get_block_ref().object_offset() + stream.get_block_ref().nb_objects();
    let mut block = stream.replace_block(Arc::new(MetricsBlock::new(
        1024,
        stream.process_id(),
        stream.stream_id(),
        next_offset,
    )));
    Arc::get_mut(&mut block)
        .with_context(|| "sole owner of freshly replaced metrics block")?
        .close();
    Ok(bytes::Bytes::from(block.encode_bin(process_info)?))
}

#[ignore]
#[tokio::test]
async fn cross_audience_injected_block_is_excluded_from_materialization() -> Result<()> {
    ensure_telemetry_guard();
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let lake = connect_to_data_lake(&connection_string, &object_store_uri).await?;
    let ingestion = WebIngestionService::new(lake.clone(), WriteAudience::new("public")?);
    let beta = WriteAudience::new("beta")?;
    let alpha = WriteAudience::new("alpha")?;

    // The victim: a process registered under "beta".
    let process_id = Uuid::new_v4();
    let process_info = make_process_info(process_id, None, HashMap::new());
    let process_body = bytes::Bytes::from(encode_cbor(&process_info)?);
    ingestion
        .insert_process(process_body, &beta)
        .await
        .with_context(|| "insert_process (victim)")?;

    // The victim's own log stream and block, both stamped "beta".
    let mut victim_log_stream =
        LogStream::new(1024, process_id, &[String::from("log")], HashMap::new());
    let victim_log_stream_info = make_stream_info(&victim_log_stream);
    ingestion
        .insert_stream(
            bytes::Bytes::from(encode_cbor(&victim_log_stream_info)?),
            &beta,
        )
        .await
        .with_context(|| "insert_stream (victim log)")?;
    let victim_log_body = make_log_block_body(&mut victim_log_stream, &process_info, "victim-log")?;
    ingestion
        .insert_block(victim_log_body, &beta)
        .await
        .with_context(|| "insert_block (victim log)")?;

    // The victim's own metrics stream and block, both stamped "beta".
    let mut victim_metrics_stream =
        MetricsStream::new(1024, process_id, &[String::from("metrics")], HashMap::new());
    let victim_metrics_stream_info = make_stream_info(&victim_metrics_stream);
    ingestion
        .insert_stream(
            bytes::Bytes::from(encode_cbor(&victim_metrics_stream_info)?),
            &beta,
        )
        .await
        .with_context(|| "insert_stream (victim metrics)")?;
    let victim_metrics_body = make_metric_block_body(
        &mut victim_metrics_stream,
        &process_info,
        &VICTIM_METRIC_DESC,
        1,
    )?;
    ingestion
        .insert_block(victim_metrics_body, &beta)
        .await
        .with_context(|| "insert_block (victim metrics)")?;

    // The attacker: registers its own fresh stream (honestly "alpha") but names the VICTIM's
    // `process_id` -- `insert_stream` accepts any `process_id` unconditionally. Every row it
    // writes is stamped "alpha", its own credential's audience, never the victim's.
    let mut attacker_log_stream =
        LogStream::new(1024, process_id, &[String::from("log")], HashMap::new());
    let attacker_log_stream_info = make_stream_info(&attacker_log_stream);
    ingestion
        .insert_stream(
            bytes::Bytes::from(encode_cbor(&attacker_log_stream_info)?),
            &alpha,
        )
        .await
        .with_context(|| "insert_stream (attacker log)")?;
    let attacker_log_body =
        make_log_block_body(&mut attacker_log_stream, &process_info, "attacker-log")?;
    ingestion
        .insert_block(attacker_log_body, &alpha)
        .await
        .with_context(|| "insert_block (attacker log)")?;

    let mut attacker_metrics_stream =
        MetricsStream::new(1024, process_id, &[String::from("metrics")], HashMap::new());
    let attacker_metrics_stream_info = make_stream_info(&attacker_metrics_stream);
    ingestion
        .insert_stream(
            bytes::Bytes::from(encode_cbor(&attacker_metrics_stream_info)?),
            &alpha,
        )
        .await
        .with_context(|| "insert_stream (attacker metrics)")?;
    let attacker_metrics_body = make_metric_block_body(
        &mut attacker_metrics_stream,
        &process_info,
        &ATTACKER_METRIC_DESC,
        999,
    )?;
    ingestion
        .insert_block(attacker_metrics_body, &alpha)
        .await
        .with_context(|| "insert_block (attacker metrics)")?;

    // Sanity check directly against Postgres: all four raw blocks (2 victim, 2 attacker) exist.
    // The exclusion this test proves happens at `blocks_view`'s *materialization*, never at
    // write time -- `insert_block` never rejects a mismatched row.
    let raw_block_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM blocks WHERE process_id = $1")
            .bind(process_id)
            .fetch_one(&lake.db_pool)
            .await
            .with_context(|| "counting raw blocks in Postgres")?;
    assert_eq!(
        raw_block_count, 4,
        "all four raw blocks must exist in Postgres regardless of the audience mismatch -- \
         insert_block never rejects a mismatched row, only materialization excludes it"
    );

    let lake = Arc::new(lake);
    let runtime = Arc::new(make_runtime_env()?);
    let lakehouse = Arc::new(LakehouseContext::new(lake.clone(), runtime.clone())?);
    let view_factory = Arc::new(
        default_view_factory(runtime.clone(), lake.clone(), lakehouse.default_audience()).await?,
    );
    let null_response_writer = Arc::new(ResponseWriter::new(None));

    let insert_begin = (Utc::now() - TimeDelta::hours(1)).duration_trunc(TimeDelta::hours(1))?;
    let insert_range = TimeRange::new(insert_begin, insert_begin + TimeDelta::hours(3));

    // Materialize in dependency order: `blocks` first (the single exclusion point), then
    // `processes`/`streams`/`log_entries`/`measures` (all read from `blocks`' own materialized
    // partitions), then `log_stats` (reads from `log_entries`).
    let blocks_view = view_factory
        .get_global_view("blocks")
        .expect("blocks global view");
    reset_global_view(
        lakehouse.clone(),
        blocks_view,
        insert_range,
        null_response_writer.clone(),
    )
    .await?;
    for view_set_name in ["processes", "streams", "log_entries", "measures"] {
        let view = view_factory
            .get_global_view(view_set_name)
            .unwrap_or_else(|| panic!("{view_set_name} global view"));
        reset_global_view(
            lakehouse.clone(),
            view,
            insert_range,
            null_response_writer.clone(),
        )
        .await?;
    }
    let log_stats_view = view_factory
        .get_global_view("log_stats")
        .expect("log_stats global view");
    reset_global_view(
        lakehouse.clone(),
        log_stats_view,
        insert_range,
        null_response_writer.clone(),
    )
    .await?;

    let part_provider = Arc::new(LivePartitionProvider::new(lake.db_pool.clone()));

    // --- `blocks`: exactly the victim's two blocks; the attacker's two are excluded ----------
    let blocks_answer = query(
        lakehouse.clone(),
        part_provider.clone(),
        Some(insert_range),
        &format!("SELECT audience FROM blocks WHERE process_id = '{process_id}'"),
        view_factory.clone(),
        Arc::new(NoOpSessionConfigurator),
        CallerContext::internal(),
    )
    .await
    .with_context(|| "querying blocks")?;
    let mut block_audiences = Vec::new();
    for batch in &blocks_answer.record_batches {
        let col = string_column_by_name(batch, "audience")?;
        for i in 0..col.len() {
            block_audiences.push(col.value(i)?.to_string());
        }
    }
    assert_eq!(
        block_audiences.len(),
        2,
        "exactly the victim's two blocks must materialize into `blocks` -- the attacker's two \
         (mismatched against the victim's `processes` row) must be excluded, got {block_audiences:?}"
    );
    assert!(
        block_audiences.iter().all(|a| a == "beta"),
        "every surviving block must carry the victim's own audience, never the attacker's, \
         got {block_audiences:?}"
    );

    // --- `log_entries`: only the victim's log entry, never the attacker's -------------------
    let log_entries_answer = query(
        lakehouse.clone(),
        part_provider.clone(),
        Some(insert_range),
        &format!("SELECT msg, audience FROM log_entries WHERE process_id = '{process_id}'"),
        view_factory.clone(),
        Arc::new(NoOpSessionConfigurator),
        CallerContext::internal(),
    )
    .await
    .with_context(|| "querying log_entries")?;
    let mut log_rows = Vec::new();
    for batch in &log_entries_answer.record_batches {
        let msg_col = string_column_by_name(batch, "msg")?;
        let audience_col = string_column_by_name(batch, "audience")?;
        for i in 0..batch.num_rows() {
            log_rows.push((
                msg_col.value(i)?.to_string(),
                audience_col.value(i)?.to_string(),
            ));
        }
    }
    assert_eq!(
        log_rows,
        vec![("victim-log".to_string(), "beta".to_string())],
        "log_entries must contain only the victim's own log entry, never the attacker's -- as a \
         consequence of blocks_view's own exclusion, not any check of log_entries' own"
    );

    // --- `measures`: only the victim's metric, never the attacker's ------------------------
    let measures_answer = query(
        lakehouse.clone(),
        part_provider.clone(),
        Some(insert_range),
        &format!("SELECT name, audience FROM measures WHERE process_id = '{process_id}'"),
        view_factory.clone(),
        Arc::new(NoOpSessionConfigurator),
        CallerContext::internal(),
    )
    .await
    .with_context(|| "querying measures")?;
    let mut measure_rows = Vec::new();
    for batch in &measures_answer.record_batches {
        let name_col = string_column_by_name(batch, "name")?;
        let audience_col = string_column_by_name(batch, "audience")?;
        for i in 0..batch.num_rows() {
            measure_rows.push((
                name_col.value(i)?.to_string(),
                audience_col.value(i)?.to_string(),
            ));
        }
    }
    assert_eq!(
        measure_rows,
        vec![("victim-metric".to_string(), "beta".to_string())],
        "measures must contain only the victim's own metric, never the attacker's -- as a \
         consequence of blocks_view's own exclusion, not any check of measures' own"
    );

    // --- `processes`/`streams`: the victim's own row keeps its own stamp --------------------
    // The excluded block never reaches the `max(audience)` aggregate these views compute over
    // `blocks`, so plain `max(audience)` (unchanged by this plan) has nothing attacker-controlled
    // left to relabel it with.
    let processes_answer = query(
        lakehouse.clone(),
        part_provider.clone(),
        Some(insert_range),
        &format!("SELECT audience FROM processes WHERE process_id = '{process_id}'"),
        view_factory.clone(),
        Arc::new(NoOpSessionConfigurator),
        CallerContext::internal(),
    )
    .await
    .with_context(|| "querying processes")?;
    let mut process_audiences = Vec::new();
    for batch in &processes_answer.record_batches {
        let col = string_column_by_name(batch, "audience")?;
        for i in 0..col.len() {
            process_audiences.push(col.value(i)?.to_string());
        }
    }
    assert_eq!(
        process_audiences,
        vec!["beta".to_string()],
        "the victim's processes row must keep audience='beta', unaffected by the attacker's \
         excluded block"
    );

    // --- `log_stats`: the victim's row keeps audience='beta', with no extra 'alpha' row ------
    // Covers the `GROUP BY audience` change in `log_stats_view.rs`: the attacker's row never
    // reached `log_entries`, so it can't be aggregated here either.
    let log_stats_answer = query(
        lakehouse.clone(),
        part_provider.clone(),
        Some(insert_range),
        &format!("SELECT audience, count FROM log_stats WHERE process_id = '{process_id}'"),
        view_factory.clone(),
        Arc::new(NoOpSessionConfigurator),
        CallerContext::internal(),
    )
    .await
    .with_context(|| "querying log_stats")?;
    let mut log_stats_rows = Vec::new();
    for batch in &log_stats_answer.record_batches {
        use micromegas_analytics::dfext::typed_column::typed_column_by_name;
        let audience_col = string_column_by_name(batch, "audience")?;
        let count_col: &datafusion::arrow::array::Int64Array =
            typed_column_by_name(batch, "count")?;
        for i in 0..batch.num_rows() {
            log_stats_rows.push((audience_col.value(i)?.to_string(), count_col.value(i)));
        }
    }
    assert_eq!(
        log_stats_rows,
        vec![("beta".to_string(), 1)],
        "log_stats must keep exactly one row for the victim's process, audience='beta', count=1 \
         -- no extra 'alpha' row from the excluded block"
    );

    Ok(())
}
