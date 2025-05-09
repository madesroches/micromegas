use anyhow::{Context, Result};
use chrono::{DurationRound, TimeDelta, Utc};
use micromegas_analytics::{
    lakehouse::{
        partition_cache::LivePartitionProvider, query::query, runtime::make_runtime_env,
        view_factory::default_view_factory,
    },
    time::TimeRange,
};
use micromegas_ingestion::data_lake_connection::connect_to_data_lake;
use micromegas_telemetry_sink::TelemetryGuardBuilder;
use micromegas_tracing::levels::LevelFilter;
use std::sync::Arc;

#[ignore]
#[tokio::test]
async fn histo_view_test() -> Result<()> {
    let _telemetry_guard = TelemetryGuardBuilder::default()
        .with_ctrlc_handling()
        .with_local_sink_max_level(LevelFilter::Info)
        .build();
    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let runtime = Arc::new(make_runtime_env()?);
    let lake = Arc::new(connect_to_data_lake(&connection_string, &object_store_uri).await?);
    let end_range = Utc::now().duration_trunc(TimeDelta::minutes(1))?;
    let begin_range = end_range - (TimeDelta::minutes(3));
    let view_factory = Arc::new(default_view_factory()?);
    let answer = query(
        runtime.clone(),
        lake.clone(),
        Arc::new(LivePartitionProvider::new(lake.db_pool.clone())),
        Some(TimeRange::new(begin_range, end_range)),
        "
        SELECT date_bin('1 minute', time) as time_bin,
               name,
               process_id,
               make_histogram(0,100,100,value) as cpu_usage_histo
        FROM   measures
        WHERE  name = 'cpu_usage'
        GROUP BY process_id, name, unit, time_bin
        ORDER BY name, time_bin, process_id;",
        view_factory.clone(),
    )
    .await?;
    let pretty_results_view =
        datafusion::arrow::util::pretty::pretty_format_batches(&answer.record_batches)?.to_string();
    eprintln!("{pretty_results_view}");

    Ok(())
}
