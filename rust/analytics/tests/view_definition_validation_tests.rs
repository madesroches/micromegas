//! Offline (no live DB) tests for `validate_view_definition` -- every check plans against a fixed
//! `log_entries`/`blocks` fixture factory, exactly like the offline harness in
//! `lakehouse_admin_gate_test.rs`.

use datafusion::arrow::datatypes::{DataType, Field, TimeUnit};
use micromegas_analytics::lakehouse::blocks_view::BlocksView;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::log_stats_view::log_stats_view_definition;
use micromegas_analytics::lakehouse::log_view::LogViewMaker;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::view::View;
use micromegas_analytics::lakehouse::view_definition::{
    ViewDefinition, ViewOptions, build_sql_batch_view, validate_view_definition,
};
use micromegas_analytics::lakehouse::view_factory::{ViewFactory, ViewMaker};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use std::sync::Arc;

async fn make_offline_lakehouse_context() -> Arc<LakehouseContext> {
    let db_pool = sqlx::PgPool::connect_lazy("postgres://user:pass@127.0.0.1:1/db")
        .expect("connect_lazy should not touch the network");
    let object_store: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::memory::InMemory::new());
    let blob_storage = Arc::new(BlobStorage::new(
        object_store,
        object_store::path::Path::from("lakehouse"),
    ));
    let lake = Arc::new(DataLakeConnection::new(db_pool, blob_storage));
    let runtime = Arc::new(make_runtime_env().expect("make_runtime_env"));
    Arc::new(LakehouseContext::new(lake, runtime).expect("LakehouseContext::new"))
}

/// `log_entries` (global, update_group 2000) and `blocks` (update_group 1000), exactly as
/// `default_view_factory` registers them -- enough for a fixture definition reading either one.
fn make_base_factory(lakehouse: &LakehouseContext) -> ViewFactory {
    let blocks_view =
        Arc::new(BlocksView::new(lakehouse.default_audience()).expect("BlocksView::new"));
    let log_view_maker = LogViewMaker {};
    let log_entries_view = log_view_maker
        .make_view("global")
        .expect("log_entries global view");
    let mut factory = ViewFactory::new(vec![log_entries_view, blocks_view]);
    factory.add_view_set("log_entries".to_string(), Arc::new(LogViewMaker {}));
    factory
}

fn base_options() -> ViewOptions {
    ViewOptions {
        min_time_column: "time_bin".to_string(),
        max_time_column: "time_bin".to_string(),
        source_partition_delta: "1 day".to_string(),
        merge_partition_delta: "1 day".to_string(),
        merge_sort_order: None,
    }
}

fn def(
    name: &str,
    extract_query: &str,
    count_src_query: &str,
    merge_partitions_query: &str,
    update_group: i32,
) -> ViewDefinition {
    ViewDefinition {
        view_set_name: name.to_string(),
        extract_query: extract_query.to_string(),
        count_src_query: count_src_query.to_string(),
        merge_partitions_query: merge_partitions_query.to_string(),
        update_group,
        options: base_options(),
    }
}

const VALID_COUNT_SRC: &str = "SELECT sum(nb_objects) as count FROM blocks \
     WHERE insert_time >= '{begin}' AND insert_time < '{end}'";

/// Runs the full validation pipeline (`build_sql_batch_view` then `validate_view_definition`),
/// exactly the order the DDL executor and registry loader both use.
async fn try_validate(
    lakehouse: &LakehouseContext,
    factory: Arc<ViewFactory>,
    definition: &ViewDefinition,
) -> anyhow::Result<()> {
    let view = build_sql_batch_view(
        definition,
        lakehouse.runtime().clone(),
        lakehouse.lake().clone(),
        factory.clone(),
        Arc::new(NoOpSessionConfigurator),
    )
    .await?;
    validate_view_definition(
        definition,
        &view,
        &factory,
        lakehouse.runtime().clone(),
        lakehouse.lake().clone(),
        Arc::new(NoOpSessionConfigurator),
    )
    .await
}

#[tokio::test]
async fn definition_with_audience_is_accepted() {
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = Arc::new(make_base_factory(&lakehouse));
    let d = def(
        "my_view",
        "SELECT date_bin('1 minute', time) as time_bin, \
                arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                count(*) as count \
         FROM log_entries \
         WHERE insert_time >= '{begin}' AND insert_time < '{end}' \
         GROUP BY time_bin",
        VALID_COUNT_SRC,
        "SELECT time_bin, arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                sum(count) as count \
         FROM {source} GROUP BY time_bin",
        2500,
    );
    try_validate(&lakehouse, factory, &d)
        .await
        .expect("a definition carrying `audience` must be accepted");
}

#[tokio::test]
async fn definition_with_process_id_only_is_accepted() {
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = Arc::new(make_base_factory(&lakehouse));
    let d = def(
        "my_view",
        "SELECT date_bin('1 minute', time) as time_bin, process_id, count(*) as count \
         FROM log_entries \
         WHERE insert_time >= '{begin}' AND insert_time < '{end}' \
         GROUP BY time_bin, process_id",
        VALID_COUNT_SRC,
        "SELECT time_bin, process_id, sum(count) as count FROM {source} \
         GROUP BY time_bin, process_id",
        2500,
    );
    try_validate(&lakehouse, factory, &d)
        .await
        .expect("a definition carrying `process_id` only must be accepted");
}

#[tokio::test]
async fn definition_with_neither_audience_nor_process_id_is_rejected() {
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = Arc::new(make_base_factory(&lakehouse));
    let d = def(
        "my_view",
        "SELECT date_bin('1 minute', time) as time_bin, count(*) as count \
         FROM log_entries \
         WHERE insert_time >= '{begin}' AND insert_time < '{end}' \
         GROUP BY time_bin",
        VALID_COUNT_SRC,
        "SELECT time_bin, sum(count) as count FROM {source} GROUP BY time_bin",
        2500,
    );
    let err = try_validate(&lakehouse, factory, &d)
        .await
        .expect_err("a definition with neither column must be rejected");
    assert!(format!("{err:#}").contains("audience"));
}

#[tokio::test]
async fn wrong_typed_audience_is_rejected_even_with_correctly_typed_process_id() {
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = Arc::new(make_base_factory(&lakehouse));
    let d = def(
        "my_view",
        "SELECT date_bin('1 minute', time) as time_bin, process_id, 1 as audience, \
                count(*) as count \
         FROM log_entries \
         WHERE insert_time >= '{begin}' AND insert_time < '{end}' \
         GROUP BY time_bin, process_id",
        VALID_COUNT_SRC,
        "SELECT time_bin, process_id, audience, sum(count) as count FROM {source} \
         GROUP BY time_bin, process_id, audience",
        2500,
    );
    let err = try_validate(&lakehouse, factory, &d)
        .await
        .expect_err("a wrong-typed `audience` must be rejected even with a valid `process_id`");
    assert!(format!("{err:#}").contains("audience"));
}

#[tokio::test]
async fn merge_query_output_schema_mismatch_is_rejected() {
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = Arc::new(make_base_factory(&lakehouse));
    let d = def(
        "my_view",
        "SELECT date_bin('1 minute', time) as time_bin, \
                arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                count(*) as count \
         FROM log_entries \
         WHERE insert_time >= '{begin}' AND insert_time < '{end}' \
         GROUP BY time_bin",
        VALID_COUNT_SRC,
        // Drops `audience` and reorders -- disagrees with the extract schema.
        "SELECT sum(count) as count, time_bin FROM {source} GROUP BY time_bin",
        2500,
    );
    let err = try_validate(&lakehouse, factory, &d)
        .await
        .expect_err("a merge query whose schema disagrees with the extract query must be rejected");
    assert!(format!("{err:#}").contains("disagrees"));
}

#[tokio::test]
async fn merge_query_that_does_not_plan_is_rejected() {
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = Arc::new(make_base_factory(&lakehouse));
    let d = def(
        "my_view",
        "SELECT date_bin('1 minute', time) as time_bin, \
                arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                count(*) as count \
         FROM log_entries \
         WHERE insert_time >= '{begin}' AND insert_time < '{end}' \
         GROUP BY time_bin",
        VALID_COUNT_SRC,
        "SELECT this_column_does_not_exist FROM {source}",
        2500,
    );
    try_validate(&lakehouse, factory, &d)
        .await
        .expect_err("a merge query that fails to plan must be rejected");
}

#[tokio::test]
async fn count_query_without_int64_count_is_rejected() {
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = Arc::new(make_base_factory(&lakehouse));
    let d = def(
        "my_view",
        "SELECT date_bin('1 minute', time) as time_bin, \
                arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                count(*) as count \
         FROM log_entries \
         WHERE insert_time >= '{begin}' AND insert_time < '{end}' \
         GROUP BY time_bin",
        "SELECT '{begin}' as begin, '{end}' as end",
        "SELECT time_bin, arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                sum(count) as count \
         FROM {source} GROUP BY time_bin",
        2500,
    );
    let err = try_validate(&lakehouse, factory, &d)
        .await
        .expect_err("a count query with no Int64 `count` column must be rejected");
    assert!(format!("{err:#}").contains("count"));
}

#[tokio::test]
async fn missing_placeholders_are_rejected() {
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = Arc::new(make_base_factory(&lakehouse));
    let d = def(
        "my_view",
        "SELECT date_bin('1 minute', time) as time_bin, \
                arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                count(*) as count \
         FROM log_entries GROUP BY time_bin",
        // No {begin}/{end}.
        "SELECT sum(nb_objects) as count FROM blocks",
        "SELECT time_bin, arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                sum(count) as count \
         FROM {source} GROUP BY time_bin",
        2500,
    );
    let err = try_validate(&lakehouse, factory, &d)
        .await
        .expect_err("count_src_query missing {begin}/{end} must be rejected");
    assert!(format!("{err:#}").contains("{begin}") || format!("{err:#}").contains("{end}"));
}

#[tokio::test]
async fn volatile_function_in_extract_query_is_rejected() {
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = Arc::new(make_base_factory(&lakehouse));
    let d = def(
        "my_view",
        "SELECT date_bin('1 minute', now()) as time_bin, \
                arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                count(*) as count \
         FROM log_entries \
         WHERE insert_time >= '{begin}' AND insert_time < '{end}' \
         GROUP BY time_bin",
        VALID_COUNT_SRC,
        "SELECT time_bin, arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                sum(count) as count \
         FROM {source} GROUP BY time_bin",
        2500,
    );
    let err = try_validate(&lakehouse, factory, &d)
        .await
        .expect_err("now() in extract_query must be rejected");
    assert!(format!("{err:#}").contains("now"));
}

#[tokio::test]
async fn embedded_ddl_in_extract_query_is_rejected() {
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = Arc::new(make_base_factory(&lakehouse));
    let d = def(
        "my_view",
        "CREATE EXTERNAL TABLE evil STORED AS CSV LOCATION 'obj://lakehouse/evil.csv'",
        VALID_COUNT_SRC,
        "SELECT * FROM {source}",
        2500,
    );
    // `build_sql_batch_view` -> `SqlBatchView::new` plans extract_query with the guarded options
    // too, so this fails at construction, before `validate_view_definition` even runs.
    try_validate(&lakehouse, factory, &d)
        .await
        .expect_err("embedded DDL in extract_query must be rejected");
}

#[tokio::test]
async fn time_column_absent_is_rejected() {
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = Arc::new(make_base_factory(&lakehouse));
    let mut d = def(
        "my_view",
        "SELECT date_bin('1 minute', time) as time_bin, \
                arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                count(*) as count \
         FROM log_entries \
         WHERE insert_time >= '{begin}' AND insert_time < '{end}' \
         GROUP BY time_bin",
        VALID_COUNT_SRC,
        "SELECT time_bin, arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                sum(count) as count \
         FROM {source} GROUP BY time_bin",
        2500,
    );
    d.options.min_time_column = "does_not_exist".to_string();
    d.options.max_time_column = "does_not_exist".to_string();
    try_validate(&lakehouse, factory, &d)
        .await
        .expect_err("a time_column absent from the schema must be rejected");
}

#[tokio::test]
async fn name_colliding_with_a_built_in_view_set_is_rejected() {
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = Arc::new(make_base_factory(&lakehouse));
    let d = def(
        "log_entries",
        "SELECT date_bin('1 minute', time) as time_bin, \
                arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                count(*) as count \
         FROM log_entries \
         WHERE insert_time >= '{begin}' AND insert_time < '{end}' \
         GROUP BY time_bin",
        VALID_COUNT_SRC,
        "SELECT time_bin, arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                sum(count) as count \
         FROM {source} GROUP BY time_bin",
        2500,
    );
    try_validate(&lakehouse, factory, &d)
        .await
        .expect_err("a name colliding with a built-in view set must be rejected");
}

#[tokio::test]
async fn update_group_ordering_check() {
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = Arc::new(make_base_factory(&lakehouse));
    let make = |group: i32| {
        def(
            "my_view",
            "SELECT date_bin('1 minute', time) as time_bin, \
                    arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                    count(*) as count \
             FROM log_entries \
             WHERE insert_time >= '{begin}' AND insert_time < '{end}' \
             GROUP BY time_bin",
            VALID_COUNT_SRC,
            "SELECT time_bin, arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                    sum(count) as count \
             FROM {source} GROUP BY time_bin",
            group,
        )
    };
    // log_entries' own update_group is 2000.
    try_validate(&lakehouse, factory.clone(), &make(2000))
        .await
        .expect_err("update_group == the read view's own group must be rejected");
    try_validate(&lakehouse, factory, &make(2001))
        .await
        .expect("update_group strictly greater than the read view's group must be accepted");
}

#[tokio::test]
async fn definition_whose_extract_query_reads_no_view_set_is_accepted() {
    // `count_src_query` still has to read a raw, `insert_time`-bearing source (`blocks`, per
    // check 5), so `update_group` is still measured against *its* group (1000) even though the
    // extract query itself reads nothing -- accepted here at 1500, comfortably above it.
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = Arc::new(make_base_factory(&lakehouse));
    let d = def(
        "my_view",
        "SELECT arrow_cast('p', 'Dictionary(Int32, Utf8)') as audience, \
                CAST('2024-01-01T00:00:00Z' AS TIMESTAMP) as time_bin, \
                CAST(1 AS BIGINT) as count",
        VALID_COUNT_SRC,
        "SELECT audience, time_bin, sum(count) as count FROM {source} GROUP BY audience, time_bin",
        1500,
    );
    try_validate(&lakehouse, factory, &d)
        .await
        .expect("a definition whose extract query reads no view set must still be accepted");
}

#[tokio::test]
async fn scanning_a_mutating_table_function_is_rejected() {
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = Arc::new(make_base_factory(&lakehouse));
    let d = def(
        "my_view",
        "SELECT * FROM retire_partitions('log_entries', 'global', \
                TIMESTAMP '2024-01-01T00:00:00Z', TIMESTAMP '2024-01-02T00:00:00Z')",
        VALID_COUNT_SRC,
        "SELECT * FROM {source}",
        2500,
    );
    // A stored scan against a mutating admin-gated table function must be rejected -- whether by
    // check 7's `TableScan` walk (which resolves and rejects it by name) or by an earlier check
    // that never reaches that far (its schema carries none of `time_bin`/`audience`/`process_id`),
    // this must not be accepted.
    try_validate(&lakehouse, factory, &d)
        .await
        .expect_err("a stored scan of retire_partitions must be rejected");
}

#[tokio::test]
async fn scanning_view_instance_is_rejected() {
    // Unlike `retire_partitions` (whose schema is unrelated to `log_entries`'s and so could be
    // rejected by an earlier, incidental check), `view_instance('log_entries', 'global')` yields
    // exactly `log_entries`'s own schema -- this definition is otherwise identical to
    // `definition_with_audience_is_accepted`'s accepted one, so it can only be caught by check 7's
    // by-name rejection of the mutating table function itself.
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = Arc::new(make_base_factory(&lakehouse));
    let d = def(
        "my_view",
        "SELECT date_bin('1 minute', time) as time_bin, \
                arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                count(*) as count \
         FROM view_instance('log_entries', 'global') \
         WHERE insert_time >= '{begin}' AND insert_time < '{end}' \
         GROUP BY time_bin",
        VALID_COUNT_SRC,
        "SELECT time_bin, arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                sum(count) as count \
         FROM {source} GROUP BY time_bin",
        2500,
    );
    try_validate(&lakehouse, factory, &d)
        .await
        .expect_err("a stored scan of view_instance must be rejected");
}

/// The validator's calibration case: the seeded `log_stats` definition must pass validation
/// unmodified, over the exact same base factory `default_view_factory` builds it against in
/// production.
#[tokio::test]
async fn seeded_log_stats_definition_passes_validation() {
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = Arc::new(make_base_factory(&lakehouse));
    let definition = log_stats_view_definition();
    try_validate(&lakehouse, factory.clone(), &definition)
        .await
        .expect("the seeded log_stats definition must pass validation unmodified");

    // The inferred schema must match today's shipped `log_stats` schema, field-for-field
    // including nullability -- a divergence (e.g. a later edit to `EXTRACT_QUERY`) would make
    // every pre-upgrade `log_stats` partition unreadable, since `partition_cache` filters
    // existing partitions on an exact `file_schema_hash` match. This is deliberately an explicit
    // expected field list rather than a comparison against `make_log_stats_view(...)`'s own
    // schema: that function is now defined as exactly `build_sql_batch_view(&log_stats_view_definition(),
    // ...)`, so comparing against it would compare the definition against itself and could never
    // catch a schema drift.
    let via_definition = build_sql_batch_view(
        &definition,
        lakehouse.runtime().clone(),
        lakehouse.lake().clone(),
        factory,
        Arc::new(NoOpSessionConfigurator),
    )
    .await
    .expect("build_sql_batch_view");
    let expected_fields = vec![
        Field::new(
            "time_bin",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            true,
        ),
        Field::new(
            "process_id",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new("level", DataType::Int32, false),
        Field::new(
            "target",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new("count", DataType::Int64, false),
        Field::new(
            "audience",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        ),
    ];
    let actual_fields: Vec<Field> = via_definition
        .get_file_schema()
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect();
    assert_eq!(
        actual_fields, expected_fields,
        "the seeded log_stats ViewDefinition's inferred schema must match today's shipped \
         log_stats schema (names, types, nullability) -- if this legitimately changed, update \
         the expected field list here and treat it as a schema-breaking migration"
    );
}
