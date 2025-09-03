use super::{sql_batch_view::SqlBatchView, view_factory::ViewFactory};
use anyhow::Result;
use chrono::TimeDelta;
use datafusion::execution::runtime_env::RuntimeEnv;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;

/// Creates a new `SqlBatchView` for log statistics aggregated by process, minute, level, and target.
pub async fn make_log_stats_view(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
) -> Result<SqlBatchView> {
    // Query to count source rows in the time range by summing nb_objects from log blocks only
    let count_src_query = Arc::new(String::from(
        r#"
        SELECT sum(nb_objects) as count
        FROM blocks
        WHERE array_has("streams.tags", 'log')
        AND insert_time >= '{begin}'
        AND insert_time < '{end}'
        ;"#,
    ));

    // Transform query to aggregate logs by time bin, process, level, and target
    let transform_query = Arc::new(String::from(
        r#"
        SELECT date_bin('1 minute', time) as time_bin,
               process_id,
               level,
               target,
               count(*) as count
        FROM log_entries
        WHERE insert_time >= '{begin}'
        AND insert_time < '{end}'
        GROUP BY process_id, level, target, time_bin
        ;"#,
    ));

    // Merge query to combine partitions
    let merge_query = Arc::new(String::from(
        r#"
        SELECT time_bin,
               process_id,
               level,
               target,
               sum(count) as count
        FROM {source}
        GROUP BY process_id, level, target, time_bin
        ;"#,
    ));

    let time_column = Arc::new(String::from("time_bin"));

    SqlBatchView::new(
        runtime,
        Arc::new("log_stats".to_owned()),
        time_column.clone(), // min_time_column
        time_column,         // max_time_column
        count_src_query,
        transform_query,
        merge_query,
        lake,
        view_factory,
        Some(3000),         // update_group
        TimeDelta::days(1), // source partition delta
        TimeDelta::days(1), // merge partition delta
        None,               // custom merger
    )
    .await
}
