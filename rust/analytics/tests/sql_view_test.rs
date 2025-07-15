use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::DurationRound;
use chrono::{TimeDelta, Utc};
use datafusion::arrow::array::{DictionaryArray, StringArray};
use datafusion::arrow::datatypes::{DataType, Field, Int16Type, Schema, TimeUnit};
use datafusion::error::DataFusionError;
use datafusion::execution::SendableRecordBatchStream;
use datafusion::execution::runtime_env::RuntimeEnv;
use datafusion::physical_plan::stream::RecordBatchReceiverStreamBuilder;
use micromegas_analytics::dfext::typed_column::typed_column_by_name;
use micromegas_analytics::lakehouse::batch_update::materialize_partition_range;
use micromegas_analytics::lakehouse::blocks_view::BlocksView;
use micromegas_analytics::lakehouse::merge::PartitionMerger;
use micromegas_analytics::lakehouse::partition::Partition;
use micromegas_analytics::lakehouse::partition_cache::{LivePartitionProvider, PartitionCache};
use micromegas_analytics::lakehouse::query::{query, query_partitions};
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::view::View;
use micromegas_analytics::lakehouse::view_factory::default_view_factory;
use micromegas_analytics::lakehouse::write_partition::retire_partitions;
use micromegas_analytics::lakehouse::{sql_batch_view::SqlBatchView, view_factory::ViewFactory};
use micromegas_analytics::response_writer::{Logger, ResponseWriter};
use micromegas_analytics::time::TimeRange;
use micromegas_ingestion::data_lake_connection::{DataLakeConnection, connect_to_data_lake};
use micromegas_telemetry_sink::TelemetryGuardBuilder;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

async fn make_log_entries_levels_per_process_minute_view(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
) -> Result<SqlBatchView> {
    let count_src_query = Arc::new(String::from(
        "
        SELECT count(*) as count
        FROM log_entries
        WHERE insert_time >= '{begin}'
        AND   insert_time < '{end}'
        ;",
    ));
    let transform_query = Arc::new(String::from(
        "
        SELECT date_bin('1 minute', time) as time_bin,
               min(time) as min_time,
               max(time) as max_time,
               process_id,
               sum(fatal) as nb_fatal,
               sum(err)   as nb_err,
               sum(warn)  as nb_warn,
               sum(info)  as nb_info,
               sum(debug) as nb_debug,
               sum(trace) as nb_trace
        FROM
          (  SELECT process_id,
                    time,
                    CAST(level==1 as INT) as fatal,
                    CAST(level==2 as INT) as err,
                    CAST(level==3 as INT) as warn,
                    CAST(level==4 as INT) as info,
                    CAST(level==5 as INT) as debug,
                    CAST(level==6 as INT) as trace
             FROM log_entries
             WHERE insert_time >= '{begin}'
             AND insert_time < '{end}'
          )
        GROUP BY process_id, time_bin
        ORDER BY time_bin, process_id;",
    ));
    let merge_partitions_query = Arc::new(String::from(
        "SELECT time_bin,
               min(min_time) as min_time,
               max(max_time) as max_time,
               process_id,
               sum(nb_fatal) as nb_fatal,
               sum(nb_err)   as nb_err,
               sum(nb_warn)  as nb_warn,
               sum(nb_info)  as nb_info,
               sum(nb_debug) as nb_debug,
               sum(nb_trace) as nb_trace
        FROM   {source}
        GROUP BY process_id, time_bin
        ORDER BY time_bin, process_id;",
    ));
    let time_column = Arc::new(String::from("time_bin"));
    SqlBatchView::new(
        runtime,
        Arc::new("log_entries_per_process_per_minute".to_owned()),
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

#[derive(Debug)]
pub struct LogSummaryMerger {
    pub runtime: Arc<RuntimeEnv>,
    pub file_schema: Arc<Schema>,
}

#[async_trait]
impl PartitionMerger for LogSummaryMerger {
    async fn execute_merge_query(
        &self,
        lake: Arc<DataLakeConnection>,
        partitions: Arc<Vec<Partition>>,
        _partitions_all_views: Arc<PartitionCache>,
    ) -> Result<SendableRecordBatchStream> {
        let processes_df = query_partitions(
            self.runtime.clone(),
            lake.clone(),
            self.file_schema.clone(),
            partitions.clone(),
            "SELECT DISTINCT process_id FROM source ORDER BY process_id;",
        )
        .await?;
        let processses_rbs = processes_df.collect().await?;
        let mut builder = RecordBatchReceiverStreamBuilder::new(self.file_schema.clone(), 10);
        for b in processses_rbs {
            let process_id_column: &DictionaryArray<Int16Type> =
                typed_column_by_name(&b, "process_id")?;
            let process_id_column: &StringArray = process_id_column
                .values()
                .as_any()
                .downcast_ref::<StringArray>()
                .with_context(|| "casting process_id column values into string array")?;
            for ir in 0..b.num_rows() {
                let process_id: &str = process_id_column.value(ir);
                let single_process_merge_query = format!(
                    "
                  SELECT time_bin,
                         min(min_time) as min_time,
                         max(max_time) as max_time,
                         process_id,
                         sum(nb_fatal) as nb_fatal,
                         sum(nb_err)   as nb_err,
                         sum(nb_warn)  as nb_warn,
                         sum(nb_info)  as nb_info,
                         sum(nb_debug) as nb_debug,
                         sum(nb_trace) as nb_trace
                  FROM   source
                  WHERE process_id = '{process_id}'
                  GROUP BY process_id, time_bin
                  ORDER BY time_bin;"
                );
                let df = query_partitions(
                    self.runtime.clone(),
                    lake.clone(),
                    self.file_schema.clone(),
                    partitions.clone(),
                    &single_process_merge_query,
                )
                .await?;
                let sender = builder.tx();
                builder.spawn(async move {
                    let rbs = df.collect().await?;
                    for rb in rbs {
                        sender.send(Ok(rb)).await.map_err(|e| {
                            DataFusionError::Execution(format!("sending record batch: {e:?}"))
                        })?;
                    }
                    Ok(())
                });
            }
        }
        Ok(builder.build())
    }
}

fn make_merger(runtime: Arc<RuntimeEnv>, file_schema: Arc<Schema>) -> Arc<dyn PartitionMerger> {
    Arc::new(LogSummaryMerger {
        runtime,
        file_schema,
    })
}

async fn make_log_entries_levels_per_process_minute_view_with_custom_merge(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
) -> Result<SqlBatchView> {
    let count_src_query = Arc::new(String::from(
        "
        SELECT count(*) as count
        FROM log_entries
        WHERE insert_time >= '{begin}'
        AND   insert_time < '{end}'
        ;",
    ));
    let transform_query = Arc::new(String::from(
        "
        SELECT date_bin('1 minute', time) as time_bin,
               min(time) as min_time,
               max(time) as max_time,
               process_id,
               sum(fatal) as nb_fatal,
               sum(err)   as nb_err,
               sum(warn)  as nb_warn,
               sum(info)  as nb_info,
               sum(debug) as nb_debug,
               sum(trace) as nb_trace
        FROM
          (  SELECT process_id,
                    time,
                    CAST(level==1 as INT) as fatal,
                    CAST(level==2 as INT) as err,
                    CAST(level==3 as INT) as warn,
                    CAST(level==4 as INT) as info,
                    CAST(level==5 as INT) as debug,
                    CAST(level==6 as INT) as trace
             FROM log_entries
             WHERE insert_time >= '{begin}'
             AND insert_time < '{end}'
          )
        GROUP BY process_id, time_bin
        ORDER BY time_bin, process_id;",
    ));
    let merge_partitions_query = Arc::new(String::from(
        "
        SELECT time_bin,
               min(min_time) as min_time,
               max(max_time) as max_time,
               process_id,
               sum(nb_fatal) as nb_fatal,
               sum(nb_err)   as nb_err,
               sum(nb_warn)  as nb_warn,
               sum(nb_info)  as nb_info,
               sum(nb_debug) as nb_debug,
               sum(nb_trace) as nb_trace
        FROM   {source}
        GROUP BY process_id, time_bin
        ORDER BY time_bin, process_id;",
    ));
    let time_column = Arc::new(String::from("time_bin"));
    SqlBatchView::new(
        runtime,
        Arc::new("log_entries_per_process_per_minute".to_owned()),
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
        Some(&make_merger),
    )
    .await
}

#[expect(clippy::too_many_arguments)]
pub async fn materialize_range(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
    log_summary_view: Arc<dyn View>,
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
    let log_entries_view = view_factory.make_view("log_entries", "global")?;
    materialize_partition_range(
        partitions.clone(),
        runtime.clone(),
        lake.clone(),
        log_entries_view,
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
        log_summary_view.clone(),
        insert_range,
        partition_time_delta / 2, // this validates that the source rows are not read twice
        logger.clone(),
    )
    .await?;
    Ok(())
}

async fn retire_existing_partitions(
    lake: Arc<DataLakeConnection>,
    log_summary_view: Arc<dyn View>,
    logger: Arc<dyn Logger>,
) -> Result<()> {
    let mut tr = lake.db_pool.begin().await?;
    let now = Utc::now();
    let begin = now - TimeDelta::days(10);
    retire_partitions(
        &mut tr,
        &log_summary_view.get_view_set_name(),
        &log_summary_view.get_view_instance_id(),
        begin,
        now,
        logger,
    )
    .await?;
    tr.commit().await.with_context(|| "commit")?;
    Ok(())
}

async fn test_log_summary_view(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    log_summary_view: Arc<SqlBatchView>,
) -> Result<()> {
    let mut view_factory = default_view_factory(runtime.clone(), lake.clone()).await?;
    view_factory.add_global_view(log_summary_view.clone());
    let view_factory = Arc::new(view_factory);
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    retire_existing_partitions(
        lake.clone(),
        log_summary_view.clone(),
        null_response_writer.clone(),
    )
    .await?;
    let ref_schema = Arc::new(Schema::new(vec![
        Field::new(
            "time_bin",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            true,
        ),
        Field::new(
            "min_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            true,
        ),
        Field::new(
            "max_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            true,
        ),
        Field::new(
            "process_id",
            DataType::Dictionary(DataType::Int16.into(), DataType::Utf8.into()),
            false,
        ),
        Field::new("nb_fatal", DataType::Int64, true),
        Field::new("nb_err", DataType::Int64, true),
        Field::new("nb_warn", DataType::Int64, true),
        Field::new("nb_info", DataType::Int64, true),
        Field::new("nb_debug", DataType::Int64, true),
        Field::new("nb_trace", DataType::Int64, true),
    ]));
    assert_eq!(log_summary_view.get_file_schema(), ref_schema);
    let ref_schema_hash: Vec<u8> = vec![105, 221, 132, 185, 221, 232, 62, 136];
    assert_eq!(log_summary_view.get_file_schema_hash(), ref_schema_hash);
    let end_range = Utc::now().duration_trunc(TimeDelta::minutes(1))?;
    let begin_range = end_range - (TimeDelta::minutes(3));
    let insert_range = TimeRange::new(begin_range, end_range);
    materialize_range(
        runtime.clone(),
        lake.clone(),
        view_factory.clone(),
        log_summary_view.clone(),
        insert_range,
        TimeDelta::seconds(10),
        null_response_writer.clone(),
    )
    .await?;
    materialize_range(
        runtime.clone(),
        lake.clone(),
        view_factory.clone(),
        log_summary_view.clone(),
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
               min_time,
               max_time,
               process_id,
               nb_fatal,
               nb_err,
               nb_warn,
               nb_info,
               nb_debug,
               nb_trace
        FROM   log_entries_per_process_per_minute
        ORDER BY time_bin, process_id;",
        view_factory.clone(),
    )
    .await?;
    let pretty_results_view =
        datafusion::arrow::util::pretty::pretty_format_batches(&answer.record_batches)?.to_string();
    eprintln!("{pretty_results_view}");

    let answer = query(
        runtime.clone(),
        lake.clone(),
        Arc::new(LivePartitionProvider::new(lake.db_pool.clone())),
        Some(TimeRange::new(begin_range, end_range)),
        "
        SELECT date_bin('1 minute', time) as time_bin,
               min(time) as min_time,
               max(time) as max_time,
               process_id,
               sum(nb_fatal) as nb_fatal,
               sum(nb_err)   as nb_err,
               sum(nb_warn)  as nb_warn,
               sum(nb_info)  as nb_info,
               sum(nb_debug) as nb_debug,
               sum(nb_trace) as nb_trace
        FROM   (
                    SELECT process_id,
                           time,
                           CAST(level==1 as INT) as nb_fatal,
                           CAST(level==2 as INT) as nb_err,
                           CAST(level==3 as INT) as nb_warn,
                           CAST(level==4 as INT) as nb_info,
                           CAST(level==5 as INT) as nb_debug,
                           CAST(level==6 as INT) as nb_trace
                    FROM log_entries
               )
        GROUP BY process_id, time_bin
        ORDER BY time_bin, process_id;",
        view_factory,
    )
    .await?;
    let pretty_results_ref =
        datafusion::arrow::util::pretty::pretty_format_batches(&answer.record_batches)?.to_string();
    eprintln!("{pretty_results_ref}");
    assert_eq!(pretty_results_view, pretty_results_ref);
    Ok(())
}

#[ignore]
#[tokio::test]
async fn sql_view_test() -> Result<()> {
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
    let log_summary_view_merge = Arc::new(
        make_log_entries_levels_per_process_minute_view_with_custom_merge(
            runtime.clone(),
            lake.clone(),
            Arc::new(default_view_factory(runtime.clone(), lake.clone()).await?),
        )
        .await?,
    );
    test_log_summary_view(runtime.clone(), lake.clone(), log_summary_view_merge).await?;

    let log_summary_view = Arc::new(
        make_log_entries_levels_per_process_minute_view(
            runtime.clone(),
            lake.clone(),
            Arc::new(default_view_factory(runtime.clone(), lake.clone()).await?),
        )
        .await?,
    );
    test_log_summary_view(runtime, lake.clone(), log_summary_view).await?;

    Ok(())
}
