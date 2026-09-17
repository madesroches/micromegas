use super::{
    session_configurator::NoOpSessionConfigurator,
    sql_batch_view::SqlBatchView,
    view_definition::{ViewDefinition, ViewOptions, build_sql_batch_view},
    view_factory::ViewFactory,
};
use anyhow::Result;
use datafusion::execution::runtime_env::RuntimeEnv;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;

/// Query to count source rows in the time range by summing nb_objects from log blocks only.
const COUNT_SRC_QUERY: &str = r#"
        SELECT sum(nb_objects) as count
        FROM blocks
        WHERE array_has("streams.tags", 'log')
        AND insert_time >= '{begin}'
        AND insert_time < '{end}'
        "#;

// Transform query to aggregate logs by time bin, process, level, and target. No ORDER BY is
// written here -- the extract path applies the sort from the with_merge_sort_order columns
// below (see sql_partition_spec::plan_sorted_extract) before recording the
// (time_bin, process_id, level, target) sort_order guarantee, never reaching this SQL text.
//
// `audience` joins the GROUP BY: `log_entries.audience` is a per-row stamp, and a single
// `process_id` can still span two audiences, so grouping on it too keeps those rows separate
// instead of letting `max(audience)` collapse them into one mislabelled row. It does **not**
// join the declared `with_merge_sort_order` columns below -- see that builder's doc comment
// for why an extra, unordered `GROUP BY` key degrades the merge query's `InputOrderMode` to
// `PartiallySorted` rather than blocking streaming aggregation outright.
const EXTRACT_QUERY: &str = r#"
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
        "#;

// Merge query to combine partitions. No ORDER BY is written here -- QueryMerger applies the
// sort as a DataFusion logical-plan node from the with_merge_sort_order columns below, never
// reaching this SQL text. `audience` joins this GROUP BY too, for the same reason as the
// transform query above.
const MERGE_QUERY: &str = r#"
        SELECT time_bin,
               process_id,
               level,
               target,
               sum(count) as count,
               arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience
        FROM {source}
        GROUP BY process_id, level, target, time_bin, audience
        "#;

/// `log_stats`'s definition, expressed as a `ViewDefinition` -- the single source of truth shared
/// by the migration seed (`migration.rs`'s `upgrade_v9_to_v10`), the parser round-trip test, and
/// [`make_log_stats_view`] below. `log_stats` is a `SqlBatchView` whose only distinction from a
/// DDL-defined view is that its SQL lives in Rust rather than in a `lakehouse_view_set_definitions`
/// row -- this function is what lets the migration seed that row.
pub fn log_stats_view_definition() -> ViewDefinition {
    ViewDefinition {
        view_set_name: "log_stats".to_string(),
        extract_query: EXTRACT_QUERY.to_string(),
        count_src_query: COUNT_SRC_QUERY.to_string(),
        merge_partitions_query: MERGE_QUERY.to_string(),
        update_group: 3000,
        options: ViewOptions {
            min_time_column: "time_bin".to_string(),
            max_time_column: "time_bin".to_string(),
            source_partition_delta: "1 day".to_string(),
            merge_partition_delta: "1 day".to_string(),
            // Time first: keeps merged partitions time-local, preserving row-group pruning on
            // time_bin for user queries. GROUP BY key order is irrelevant to streaming, so any
            // prefix of these four columns would stream too -- this is the full declared order.
            merge_sort_order: Some(vec![
                "time_bin".to_string(),
                "process_id".to_string(),
                "level".to_string(),
                "target".to_string(),
            ]),
        },
    }
}

/// Assembles `log_stats_view_definition()`'s equivalent `CREATE MATERIALIZED VIEW log_stats
/// WITH (...)` DDL text, for the migration seed's `definition_sql` column (kept for display and
/// audit only -- never re-parsed). Each query is wrapped in `$$...$$`; none of them contains a
/// `$$`, so no quote-escaping helper is needed.
pub fn log_stats_ddl_text() -> String {
    let def = log_stats_view_definition();
    format!(
        "CREATE MATERIALIZED VIEW {} WITH (\n\
         \x20 extract_query = $${}$$,\n\
         \x20 count_src_query = $${}$$,\n\
         \x20 merge_partitions_query = $${}$$,\n\
         \x20 update_group = {},\n\
         \x20 time_column = '{}',\n\
         \x20 source_partition_delta = '{}',\n\
         \x20 merge_partition_delta = '{}',\n\
         \x20 merge_sort_order = '{}'\n\
         )",
        def.view_set_name,
        def.extract_query,
        def.count_src_query,
        def.merge_partitions_query,
        def.update_group,
        def.options.min_time_column,
        def.options.source_partition_delta,
        def.options.merge_partition_delta,
        def.options
            .merge_sort_order
            .as_ref()
            .map(|cols| cols.join(", "))
            .unwrap_or_default(),
    )
}

/// Creates a new `SqlBatchView` for log statistics aggregated by process, minute, level, and
/// target -- `build_sql_batch_view` over [`log_stats_view_definition`], so `log_stats` is built
/// through the exact same path a DDL-defined view is.
pub async fn make_log_stats_view(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
) -> Result<SqlBatchView> {
    build_sql_batch_view(
        &log_stats_view_definition(),
        runtime,
        lake,
        view_factory,
        Arc::new(NoOpSessionConfigurator),
    )
    .await
}
