// Regression test for #1393: a failing view must not prevent other views in
// the same update group from being materialized by
// `micromegas::servers::maintenance::materialize_all_views`. Requires a live
// Postgres + object store (`MICROMEGAS_SQL_CONNECTION_STRING` /
// `MICROMEGAS_OBJECT_STORE_URI`), so it's marked `#[ignore]`.
use anyhow::{Context, Result};
use chrono::{DurationRound, TimeDelta, Utc};
use micromegas::analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas::analytics::lakehouse::partition_cache::LivePartitionProvider;
use micromegas::analytics::lakehouse::query::query;
use micromegas::analytics::lakehouse::runtime::make_runtime_env;
use micromegas::analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas::analytics::lakehouse::sql_batch_view::SqlBatchView;
use micromegas::analytics::lakehouse::view::View;
use micromegas::analytics::lakehouse::view_factory::{ViewFactory, default_view_factory};
use micromegas::analytics::time::TimeRange;
use micromegas::datafusion::arrow::array::Int64Array;
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
    update_group: i32,
) -> Result<SqlBatchView> {
    let count_src_query = Arc::new(String::from(
        "SELECT count(*) as count FROM this_table_does_not_exist_1393
         WHERE insert_time >= '{begin}' AND insert_time < '{end}';",
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
        Arc::new(String::from("materialize_fail_isolation_failing_view")),
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

/// A trivial, always-valid view: counts/copies rows from `log_entries`.
async fn make_succeeding_view(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
    update_group: i32,
) -> Result<SqlBatchView> {
    let count_src_query = Arc::new(String::from(
        "SELECT count(*) as count FROM log_entries
         WHERE insert_time >= '{begin}' AND insert_time < '{end}';",
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
        Arc::new(String::from("materialize_fail_isolation_succeeding_view")),
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

    // Both views land in the same update group: same-group views must have
    // no inter-dependencies, so isolating one's failure from the other's is
    // always safe (see maintenance.rs's update-group comment).
    let update_group = 4242;
    let view_factory = Arc::new(default_view_factory(runtime.clone(), lake.clone()).await?);
    let failing_view: Arc<dyn View> = Arc::new(
        make_failing_view(
            runtime.clone(),
            lake.clone(),
            view_factory.clone(),
            update_group,
        )
        .await?,
    );
    let succeeding_view: Arc<dyn View> = Arc::new(
        make_succeeding_view(
            runtime.clone(),
            lake.clone(),
            view_factory.clone(),
            update_group,
        )
        .await?,
    );

    let end_range = Utc::now().duration_trunc(TimeDelta::minutes(1))?;
    let begin_range = end_range - TimeDelta::minutes(3);
    let insert_range = TimeRange::new(begin_range, end_range);
    let views = Arc::new(vec![failing_view, succeeding_view]);

    let result = materialize_all_views(
        lakehouse.clone(),
        views,
        insert_range,
        TimeDelta::minutes(1),
    )
    .await;
    assert!(
        result.is_err(),
        "materialize_all_views should report the failing view's error"
    );

    let answer = query(
        lakehouse,
        Arc::new(LivePartitionProvider::new(lake.db_pool.clone())),
        Some(insert_range),
        "SELECT count(*) as nb_partitions FROM list_partitions()
         WHERE view_set_name = 'materialize_fail_isolation_succeeding_view'
         AND view_instance_id = 'global';",
        view_factory,
        Arc::new(NoOpSessionConfigurator),
        false,
    )
    .await?;
    let nb_partitions: i64 = answer
        .record_batches
        .first()
        .with_context(|| "expected at least one record batch")?
        .column(0)
        .as_any()
        .downcast_ref::<Int64Array>()
        .with_context(|| "expected an Int64Array for nb_partitions")?
        .value(0);
    assert!(
        nb_partitions > 0,
        "the succeeding view should have produced a partition despite the failing view's error"
    );

    Ok(())
}
