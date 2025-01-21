use anyhow::{Context, Result};
use chrono::DurationRound;
use chrono::{TimeDelta, Utc};
use datafusion::arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use micromegas_analytics::lakehouse::batch_update::materialize_partition_range;
use micromegas_analytics::lakehouse::blocks_view::BlocksView;
use micromegas_analytics::lakehouse::partition_cache::{LivePartitionProvider, PartitionCache};
use micromegas_analytics::lakehouse::query::query_single_view;
use micromegas_analytics::lakehouse::view::View;
use micromegas_analytics::lakehouse::view_factory::default_view_factory;
use micromegas_analytics::lakehouse::{sql_batch_view::SqlBatchView, view_factory::ViewFactory};
use micromegas_analytics::response_writer::ResponseWriter;
use micromegas_ingestion::data_lake_connection::{connect_to_data_lake, DataLakeConnection};
use micromegas_telemetry_sink::TelemetryGuardBuilder;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

async fn make_log_entries_levels_per_process_view(
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
) -> Result<SqlBatchView> {
    let src_query = Arc::new(String::from(
        "
        SELECT process_id,
               date_bin('1 minute', time) as time_bin,
               CAST(level==1 as INT) as fatal,
               CAST(level==2 as INT) as err,
               CAST(level==3 as INT) as warn,
               CAST(level==4 as INT) as info,
               CAST(level==5 as INT) as debug,
               CAST(level==6 as INT) as trace
        FROM log_entries;",
    ));

    let transform_query = Arc::new(String::from(
        "
        SELECT time_bin,
               process_id,
               sum(fatal) as nb_fatal,
               sum(err)   as nb_err,
               sum(warn)  as nb_warn,
               sum(info)  as nb_info,
               sum(debug) as nb_debug,
               sum(trace) as nb_trace
        FROM   source
        GROUP BY process_id, time_bin
        ORDER BY time_bin, process_id;",
    ));

    let merge_partitions_query = Arc::new(String::from(
        "
        SELECT time_bin,
               process_id,
               sum(nb_fatal) as nb_fatal,
               sum(nb_err)   as nb_err,
               sum(nb_warn)  as nb_warn,
               sum(nb_info)  as nb_info,
               sum(nb_debug) as nb_debug,
               sum(nb_trace) as nb_trace
        FROM   src
        GROUP BY process_id, time_bin
        ORDER BY time_bin, process_id;",
    ));

    SqlBatchView::new(
        Arc::new("log_entries_per_process".to_owned()),
        Arc::new("global".to_owned()),
        Arc::new("time_bin".to_owned()),
        src_query,
        transform_query,
        merge_partitions_query,
        lake,
        view_factory,
    )
    .await
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
    let lake = Arc::new(connect_to_data_lake(&connection_string, &object_store_uri).await?);
    let view_factory = Arc::new(default_view_factory()?);
    let log_summary_view = Arc::new(
        make_log_entries_levels_per_process_view(lake.clone(), view_factory.clone()).await?,
    );
    let ref_schema = Arc::new(Schema::new(vec![
        Field::new(
            "time_bin",
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

    let ref_schema_hash: Vec<u8> = vec![219, 37, 165, 158, 123, 73, 39, 204];
    assert_eq!(log_summary_view.get_file_schema_hash(), ref_schema_hash);

    let nb_partitions = 3;
    let partition_time_delta = TimeDelta::minutes(1);
    let now = Utc::now();
    let end_range = now.duration_trunc(partition_time_delta)?;
    let begin_range = end_range - (partition_time_delta * nb_partitions);

    let mut partitions = Arc::new(
        // we only need the blocks partitions for this call
        PartitionCache::fetch_overlapping_insert_range(&lake.db_pool, begin_range, end_range)
            .await?,
    );
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    let blocks_view = Arc::new(BlocksView::new()?);
    materialize_partition_range(
        partitions.clone(),
        lake.clone(),
        blocks_view,
        begin_range,
        end_range,
        partition_time_delta,
        null_response_writer.clone(),
    )
    .await?;
    partitions = Arc::new(
        PartitionCache::fetch_overlapping_insert_range(&lake.db_pool, begin_range, end_range)
            .await?,
    );
    let log_entries_view = view_factory.make_view("log_entries", "global")?;
    materialize_partition_range(
        partitions.clone(),
        lake.clone(),
        log_entries_view,
        begin_range,
        end_range,
        partition_time_delta,
        null_response_writer.clone(),
    )
    .await?;
    partitions = Arc::new(
        PartitionCache::fetch_overlapping_insert_range(&lake.db_pool, begin_range, end_range)
            .await?,
    );
    materialize_partition_range(
        partitions.clone(),
        lake.clone(),
        log_summary_view.clone(),
        begin_range,
        end_range,
        partition_time_delta,
        null_response_writer.clone(),
    )
    .await?;

    let answer = query_single_view(
        lake.clone(),
        Arc::new(LivePartitionProvider::new(lake.db_pool.clone())),
        None,
        "SELECT * FROM log_entries_per_process;",
        log_summary_view,
    )
    .await?;
    let pretty_results =
        datafusion::arrow::util::pretty::pretty_format_batches(&answer.record_batches)?.to_string();
    eprintln!("{pretty_results}");

    info!("bye");

    Ok(())
}
