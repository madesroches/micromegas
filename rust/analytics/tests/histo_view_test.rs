use anyhow::{Context, Result};
use chrono::{DurationRound, TimeDelta, Utc};
use datafusion::execution::runtime_env::RuntimeEnv;
use micromegas_analytics::{
    lakehouse::{
        batch_update::materialize_partition_range,
        blocks_view::BlocksView,
        partition_cache::{LivePartitionProvider, PartitionCache},
        query::query,
        runtime::make_runtime_env,
        sql_batch_view::SqlBatchView,
        view::View,
        view_factory::{default_view_factory, ViewFactory},
        write_partition::retire_partitions,
    },
    response_writer::{Logger, ResponseWriter},
    time::TimeRange,
};
use micromegas_ingestion::data_lake_connection::{connect_to_data_lake, DataLakeConnection};
use micromegas_telemetry_sink::TelemetryGuardBuilder;
use micromegas_tracing::levels::LevelFilter;
use std::sync::Arc;

async fn make_cpu_usage_per_process_per_minute_view(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
) -> Result<SqlBatchView> {
    let count_src_query = Arc::new(String::from(
        r#"
        SELECT sum(nb_objects) as count
        FROM   blocks
        WHERE  insert_time >= '{begin}'
        AND    insert_time < '{end}'
        AND    array_has("streams.tags", 'metrics')
        ;"#,
    ));
    let transform_query = Arc::new(String::from(
        "
        SELECT date_bin('1 minute', time) as time_bin,
               process_id,
               make_histogram(0,100,100,value) as cpu_usage_histo
        FROM   measures
        WHERE  name = 'cpu_usage'
        GROUP BY process_id, name, unit, time_bin
        ORDER BY name, time_bin, process_id;",
    ));
    let merge_partitions_query = Arc::new(String::from(
        "SELECT time_bin,
                process_id,
                sum_histograms(cpu_usage_histo) as cpu_usage_histo
        FROM   {source}
        GROUP BY time_bin, process_id
        ORDER BY time_bin, process_id;",
    ));
    let time_column = Arc::new(String::from("time_bin"));
    SqlBatchView::new(
        runtime,
        Arc::new("cpu_usage_per_process_per_minute".to_owned()),
        time_column.clone(),
        time_column,
        count_src_query,
        transform_query,
        merge_partitions_query,
        lake,
        view_factory,
        Some(4000),
        TimeDelta::days(1),
        TimeDelta::days(1),
        None,
    )
    .await
}

async fn retire_existing_partitions(
    lake: Arc<DataLakeConnection>,
    view: Arc<dyn View>,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let mut tr = lake.db_pool.begin().await?;
    let now = Utc::now();
    let begin = now - TimeDelta::days(10);
    retire_partitions(
        &mut tr,
        &view.get_view_set_name(),
        &view.get_view_instance_id(),
        begin,
        now,
        logger,
    )
    .await?;
    tr.commit().await.with_context(|| "commit")?;
    Ok(())
}

#[expect(clippy::too_many_arguments)]
pub async fn materialize_range(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
    cpu_usage_view: Arc<dyn View>,
    insert_range: TimeRange,
    partition_time_delta: TimeDelta,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let blocks_view = Arc::new(BlocksView::new()?);
    let mut partitions = Arc::new(
        PartitionCache::fetch_overlapping_insert_range_for_view(
            &lake.db_pool,
            blocks_view.get_view_set_name(),
            blocks_view.get_view_instance_id(),
            insert_range,
        )
        .await?,
    );
    materialize_partition_range(
        partitions.clone(),
        runtime.clone(),
        lake.clone(),
        blocks_view,
        insert_range,
        partition_time_delta,
        logger.clone(),
    )
    .await?;
    partitions = Arc::new(
        PartitionCache::fetch_overlapping_insert_range(&lake.db_pool, insert_range).await?,
    );
    let measures_view = view_factory.make_view("measures", "global")?;
    materialize_partition_range(
        partitions.clone(),
        runtime.clone(),
        lake.clone(),
        measures_view,
        insert_range,
        partition_time_delta,
        logger.clone(),
    )
    .await?;
    partitions = Arc::new(
        PartitionCache::fetch_overlapping_insert_range(&lake.db_pool, insert_range).await?,
    );
    materialize_partition_range(
        partitions.clone(),
        runtime.clone(),
        lake.clone(),
        cpu_usage_view.clone(),
        insert_range,
        partition_time_delta / 2, // this validates that the source rows are not read twice
        logger.clone(),
    )
    .await?;
    Ok(())
}

async fn test_cpu_usage_view(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    cpu_usage_view: Arc<SqlBatchView>,
) -> Result<()> {
    let mut view_factory = default_view_factory(runtime.clone(), lake.clone()).await?;
    view_factory.add_global_view(cpu_usage_view.clone());
    let view_factory = Arc::new(view_factory);
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    retire_existing_partitions(
        lake.clone(),
        cpu_usage_view.clone(),
        null_response_writer.clone(),
    )
    .await?;

    let schema = cpu_usage_view.get_file_schema();
    eprintln!("schema: {schema:?}");

    let end_range = Utc::now().duration_trunc(TimeDelta::minutes(1))?;
    let begin_range = end_range - (TimeDelta::minutes(3));
    let insert_range = TimeRange::new(begin_range, end_range);
    materialize_range(
        runtime.clone(),
        lake.clone(),
        view_factory.clone(),
        cpu_usage_view.clone(),
        insert_range,
        TimeDelta::seconds(10),
        null_response_writer.clone(),
    )
    .await?;
    materialize_range(
        runtime.clone(),
        lake.clone(),
        view_factory.clone(),
        cpu_usage_view.clone(),
        insert_range,
        TimeDelta::minutes(1),
        null_response_writer.clone(),
    )
    .await?;

    let answer = query(
        runtime.clone(),
        lake.clone(),
        Arc::new(LivePartitionProvider::new(lake.db_pool.clone())),
        Some(TimeRange::new(begin_range, end_range)),
        "
        SELECT time_bin,
               process_id,
               quantile_from_histogram(cpu_usage_histo, 0.5)
        FROM   cpu_usage_per_process_per_minute
        ORDER BY time_bin, process_id;",
        view_factory.clone(),
    )
    .await?;
    let pretty_results_view =
        datafusion::arrow::util::pretty::pretty_format_batches(&answer.record_batches)?.to_string();
    eprintln!("{pretty_results_view}");
    Ok(())
}

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
    let cpu_usage_view = Arc::new(
        make_cpu_usage_per_process_per_minute_view(
            runtime.clone(),
            lake.clone(),
            Arc::new(default_view_factory(runtime.clone(), lake.clone()).await?),
        )
        .await?,
    );
    test_cpu_usage_view(runtime.clone(), lake.clone(), cpu_usage_view).await?;
    Ok(())
}
