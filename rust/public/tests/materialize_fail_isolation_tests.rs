// Regression test for #1393: a failing view must not prevent other views in
// the same update group from being materialized by
// `micromegas::servers::maintenance::materialize_all_views`. Requires a live
// Postgres + object store (`MICROMEGAS_SQL_CONNECTION_STRING` /
// `MICROMEGAS_OBJECT_STORE_URI`), so it's marked `#[ignore]`.
use anyhow::{Context, Result};
use chrono::{DurationRound, TimeDelta, Utc};
use micromegas::analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas::analytics::lakehouse::runtime::make_runtime_env;
use micromegas::analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas::analytics::lakehouse::sql_batch_view::SqlBatchView;
use micromegas::analytics::lakehouse::view::View;
use micromegas::analytics::lakehouse::view_factory::{ViewFactory, default_view_factory};
use micromegas::analytics::time::TimeRange;
use micromegas::datafusion::execution::runtime_env::RuntimeEnv;
use micromegas::ingestion::data_lake_connection::{DataLakeConnection, connect_to_data_lake};
use micromegas::servers::maintenance::materialize_all_views;
use micromegas_telemetry_sink::TelemetryGuardBuilder;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// A view whose `count_src_query` always errors (selects from a nonexistent
/// table), but whose `extract_query` is valid — `SqlBatchView::new` runs
/// `extract_query` unconditionally at construction time to derive the
/// schema, independently of `count_src_query`.
async fn make_failing_view(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
    view_set_name: &str,
    nonexistent_table: &str,
    update_group: i32,
) -> Result<SqlBatchView> {
    let count_src_query = Arc::new(format!(
        "SELECT count(*) as count FROM {nonexistent_table}
         WHERE insert_time >= '{{begin}}' AND insert_time < '{{end}}';",
    ));
    let extract_query = Arc::new(String::from(
        "SELECT time, process_id FROM log_entries
         WHERE insert_time >= '{begin}' AND insert_time < '{end}';",
    ));
    let merge_partitions_query = Arc::new(String::from(
        "SELECT time, process_id FROM {source} ORDER BY time;",
    ));
    let time_column = Arc::new(String::from("time"));
    SqlBatchView::new(
        runtime,
        Arc::new(view_set_name.to_owned()),
        time_column.clone(),
        time_column,
        count_src_query,
        extract_query,
        merge_partitions_query,
        lake,
        view_factory,
        Arc::new(NoOpSessionConfigurator),
        Some(update_group),
        TimeDelta::days(1),
        TimeDelta::days(1),
        None,
    )
    .await
}

/// Two views in the same update group both fail to materialize; the pass
/// must still attempt both (same-group views have no inter-dependencies, see
/// `maintenance.rs`'s update-group comment) and report both failures, not
/// just the first one encountered.
#[ignore]
#[tokio::test]
async fn materialize_all_views_isolates_same_group_failures() -> Result<()> {
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
    let lakehouse = Arc::new(LakehouseContext::new(lake.clone(), runtime.clone()));

    let update_group = 4242;
    let view_factory = Arc::new(default_view_factory(runtime.clone(), lake.clone()).await?);
    let first_view: Arc<dyn View> = Arc::new(
        make_failing_view(
            runtime.clone(),
            lake.clone(),
            view_factory.clone(),
            "materialize_fail_isolation_first_view",
            "this_table_does_not_exist_1393_a",
            update_group,
        )
        .await?,
    );
    let second_view: Arc<dyn View> = Arc::new(
        make_failing_view(
            runtime.clone(),
            lake.clone(),
            view_factory.clone(),
            "materialize_fail_isolation_second_view",
            "this_table_does_not_exist_1393_b",
            update_group,
        )
        .await?,
    );

    let end_range = Utc::now().duration_trunc(TimeDelta::minutes(1))?;
    let begin_range = end_range - TimeDelta::minutes(3);
    let insert_range = TimeRange::new(begin_range, end_range);
    let views = Arc::new(vec![first_view, second_view]);

    let err = materialize_all_views(lakehouse, views, insert_range, TimeDelta::minutes(1))
        .await
        .expect_err("both views should fail to materialize");
    let message = format!("{err:?}");
    assert!(
        message.contains("materialize_fail_isolation_first_view"),
        "expected the first view's failure to be reported, got: {message}"
    );
    assert!(
        message.contains("materialize_fail_isolation_second_view"),
        "expected the second view's failure to be reported too, not just the first one \
         encountered -- a fail-fast regression would abort before reaching it, got: {message}"
    );

    Ok(())
}
