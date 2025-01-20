use anyhow::{Context, Result};
use datafusion::arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use micromegas_analytics::lakehouse::view::View;
use micromegas_analytics::lakehouse::view_factory::default_view_factory;
use micromegas_analytics::lakehouse::{sql_batch_view::SqlBatchView, view_factory::ViewFactory};
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
    let log_summary = make_log_entries_levels_per_process_view(lake, view_factory).await?;
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

    assert_eq!(log_summary.get_file_schema(), ref_schema);

    let ref_schema_hash: Vec<u8> = vec![219, 37, 165, 158, 123, 73, 39, 204];
    assert_eq!(log_summary.get_file_schema_hash(), ref_schema_hash);

    Ok(())
}
