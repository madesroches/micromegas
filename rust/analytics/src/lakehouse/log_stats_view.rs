use super::{
    session_configurator::NoOpSessionConfigurator, sql_batch_view::SqlBatchView,
    view_factory::ViewFactory,
};
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

    // Transform query to aggregate logs by time bin, process, level, and target. The top-level
    // ORDER BY lets the fresh-write path record the (time_bin, process_id, level, target)
    // sort_order guarantee with_merge_sort_order below declares.
    //
    // `audience` joins the GROUP BY: `log_entries.audience` is a per-row stamp, and a single
    // `process_id` can still span two audiences, so grouping on it too keeps those rows separate
    // instead of letting `max(audience)` collapse them into one mislabelled row. It does **not**
    // join the declared `ORDER BY`/`with_merge_sort_order` columns below -- see that builder's
    // doc comment for why an extra, unordered `GROUP BY` key degrades the merge query's
    // `InputOrderMode` to `PartiallySorted` rather than blocking streaming aggregation outright.
    let transform_query = Arc::new(String::from(
        r#"
        SELECT date_bin('1 minute', time) as time_bin,
               process_id,
               level,
               target,
               count(*) as count,
               arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience
        FROM log_entries
        WHERE insert_time >= '{begin}'
        AND insert_time < '{end}'
        GROUP BY process_id, level, target, time_bin, audience
        ORDER BY time_bin, process_id, level, target
        ;"#,
    ));

    // Merge query to combine partitions. No ORDER BY is written here -- QueryMerger applies the
    // sort as a DataFusion logical-plan node from the with_merge_sort_order columns below, never
    // reaching this SQL text. `audience` joins this GROUP BY too, for the same reason as the
    // transform query above.
    let merge_query = Arc::new(String::from(
        r#"
        SELECT time_bin,
               process_id,
               level,
               target,
               sum(count) as count,
               arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience
        FROM {source}
        GROUP BY process_id, level, target, time_bin, audience
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
        Arc::new(NoOpSessionConfigurator),
        Some(3000),         // update_group
        TimeDelta::days(1), // source partition delta
        TimeDelta::days(1), // merge partition delta
        None,               // custom merger
    )
    .await?
    // Time first: keeps merged partitions time-local, preserving row-group pruning on time_bin
    // for user queries. GROUP BY key order is irrelevant to streaming, so any prefix of these
    // four columns would stream too -- this is the full declared order.
    .with_merge_sort_order(vec![
        Arc::new("time_bin".to_owned()),
        Arc::new("process_id".to_owned()),
        Arc::new("level".to_owned()),
        Arc::new("target".to_owned()),
    ])
}
