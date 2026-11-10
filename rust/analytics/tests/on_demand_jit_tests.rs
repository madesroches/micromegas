//! Offline (no live DB) tests for the on-demand JIT driver's pure decisions (`plan_units`,
//! `judge_unit`, `promotion_targets`) and for the rejection of on-demand views by
//! `materialize_partitions` / `regenerate_partitions`.

use chrono::{DateTime, TimeDelta, Utc};
use datafusion::arrow::array::StringArray;
use micromegas_analytics::lakehouse::blocks_view::BlocksView;
use micromegas_analytics::lakehouse::jit_partitions::PartitionFreshnessRow;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::log_view::LogViewMaker;
use micromegas_analytics::lakehouse::on_demand_jit::{
    Unit, UnitVerdict, judge_unit, plan_units, promotion_targets,
};
use micromegas_analytics::lakehouse::partition_cache::NullPartitionProvider;
use micromegas_analytics::lakehouse::partition_ladder::{LADDER, PlannedBucket};
use micromegas_analytics::lakehouse::query::{SessionRange, make_session_context};
use micromegas_analytics::lakehouse::read_scope::CallerContext;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::view_definition::{
    ViewDefinition, ViewOptions, build_sql_batch_view,
};
use micromegas_analytics::lakehouse::view_factory::{ViewFactory, ViewMaker};
use micromegas_analytics::time::TimeRange;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use std::sync::Arc;

const FIVE_MINUTES: TimeDelta = LADDER[1];
const HOUR: TimeDelta = LADDER[2];
const SCHEMA: [u8; 1] = [7];

fn t(rfc3339: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(rfc3339).unwrap().to_utc()
}

fn base() -> DateTime<Utc> {
    t("2024-01-01T10:00:00Z")
}

fn range(begin_minutes: i64, end_minutes: i64) -> TimeRange {
    TimeRange::new(
        base() + TimeDelta::minutes(begin_minutes),
        base() + TimeDelta::minutes(end_minutes),
    )
}

fn bucket(r: TimeRange, rung: TimeDelta, closed: bool) -> PlannedBucket {
    PlannedBucket {
        range: r,
        rung,
        closed,
        nb_blocks: 0,
        nb_objects: 0,
    }
}

fn row(r: TimeRange, count: i64, schema: &[u8]) -> PartitionFreshnessRow {
    PartitionFreshnessRow {
        begin_insert_time: r.begin,
        end_insert_time: r.end,
        file_schema_hash: schema.to_vec(),
        source_data_hash: count.to_le_bytes().to_vec(),
    }
}

fn hour_bucket(closed: bool) -> PlannedBucket {
    bucket(range(0, 60), HOUR, closed)
}

fn minute_rows(nb: i64, count: i64) -> Vec<PartitionFreshnessRow> {
    (0..nb)
        .map(|i| row(range(i, i + 1), count, &SCHEMA))
        .collect()
}

#[test]
fn empty_bucket_is_a_unit_without_rows_and_is_stale() {
    let units = plan_units(&[hour_bucket(true)], &[]);
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].range, range(0, 60));
    assert!(units[0].rows.is_empty());
    assert_eq!(units[0].planned_rung, Some(HOUR));
    assert_eq!(judge_unit(&units[0], &[], &SCHEMA, 0), UnitVerdict::Stale);
}

#[test]
fn exact_row_matching_the_count_is_covered() {
    let rows = vec![row(range(0, 60), 42, &SCHEMA)];
    let units = plan_units(&[hour_bucket(true)], &rows);
    assert_eq!(units[0].rows, vec![0]);
    assert_eq!(
        judge_unit(&units[0], &rows, &SCHEMA, 42),
        UnitVerdict::Covered
    );
}

#[test]
fn an_empty_partition_row_covers_a_zero_count() {
    let rows = vec![row(range(0, 60), 0, &SCHEMA)];
    let units = plan_units(&[hour_bucket(true)], &rows);
    assert_eq!(
        judge_unit(&units[0], &rows, &SCHEMA, 0),
        UnitVerdict::Covered
    );
}

#[test]
fn count_mismatch_is_stale() {
    let rows = vec![row(range(0, 60), 42, &SCHEMA)];
    let units = plan_units(&[hour_bucket(true)], &rows);
    assert_eq!(
        judge_unit(&units[0], &rows, &SCHEMA, 43),
        UnitVerdict::Stale
    );
    assert_eq!(
        judge_unit(&units[0], &rows, &SCHEMA, 41),
        UnitVerdict::Stale
    );
}

#[test]
fn fine_rows_summing_to_the_count_are_covered() {
    let rows = minute_rows(60, 10);
    let units = plan_units(&[hour_bucket(true)], &rows);
    assert_eq!(units[0].rows.len(), 60);
    assert_eq!(
        judge_unit(&units[0], &rows, &SCHEMA, 600),
        UnitVerdict::Covered
    );
    assert_eq!(
        judge_unit(&units[0], &rows, &SCHEMA, 601),
        UnitVerdict::Stale
    );
}

#[test]
fn coarser_row_grows_the_unit_and_covers_several_planned_buckets() {
    let rows = vec![row(range(0, 60), 100, &SCHEMA)];
    let planned = [
        bucket(range(0, 5), FIVE_MINUTES, true),
        bucket(range(5, 10), FIVE_MINUTES, true),
    ];
    let units = plan_units(&planned, &rows);
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].range, range(0, 60));
    assert_eq!(units[0].rows, vec![0]);
    assert_eq!(units[0].planned_rung, None);
    assert!(units[0].closed);
}

#[test]
fn misaligned_straddling_row_grows_and_coalesces_neighbors() {
    let rows = vec![row(range(55, 125), 7, &SCHEMA)];
    let planned = [
        bucket(range(0, 60), HOUR, true),
        bucket(range(60, 120), HOUR, true),
        bucket(range(120, 180), HOUR, false),
    ];
    let units = plan_units(&planned, &rows);
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].range, range(0, 180));
    assert_eq!(units[0].rows, vec![0]);
    assert!(
        !units[0].closed,
        "an absorbed open bucket keeps the unit open"
    );
    assert_eq!(units[0].planned_rung, None);
}

#[test]
fn schema_hash_mismatch_is_stale() {
    let rows = vec![row(range(0, 60), 42, &[9])];
    let units = plan_units(&[hour_bucket(true)], &rows);
    assert_eq!(units[0].rows, vec![0]);
    assert_eq!(
        judge_unit(&units[0], &rows, &SCHEMA, 42),
        UnitVerdict::Stale
    );
}

#[test]
fn touching_neighbor_rows_are_not_absorbed() {
    let rows = vec![
        row(range(-60, 0), 5, &SCHEMA),
        row(range(0, 60), 42, &SCHEMA),
        row(range(60, 120), 5, &SCHEMA),
    ];
    let units = plan_units(&[hour_bucket(true)], &rows);
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].range, range(0, 60));
    assert_eq!(units[0].rows, vec![1]);
    assert_eq!(units[0].planned_rung, Some(HOUR));
    assert_eq!(
        judge_unit(&units[0], &rows, &SCHEMA, 42),
        UnitVerdict::Covered
    );
}

fn targets_for(
    unit: &Unit,
    rows: &[PartitionFreshnessRow],
    verdict: UnitVerdict,
    rung: TimeDelta,
) -> usize {
    promotion_targets(std::slice::from_ref(unit), rows, &[verdict], rung).len()
}

#[test]
fn promotion_targets_eligibility() {
    let rows = minute_rows(60, 10);
    let covered = UnitVerdict::Covered;

    let closed = plan_units(&[hour_bucket(true)], &rows).remove(0);
    assert_eq!(targets_for(&closed, &rows, covered, HOUR), 1);
    let targets = promotion_targets(std::slice::from_ref(&closed), &rows, &[covered], HOUR);
    assert_eq!(targets[0].bucket, range(0, 60));
    assert_eq!(targets[0].inputs.len(), 60);

    // open
    let open = plan_units(&[hour_bucket(false)], &rows).remove(0);
    assert_eq!(targets_for(&open, &rows, covered, HOUR), 0);
    // not at the query's rung
    assert_eq!(targets_for(&closed, &rows, covered, FIVE_MINUTES), 0);
    // stale
    assert_eq!(targets_for(&closed, &rows, UnitVerdict::Stale, HOUR), 0);
    // fewer than two rows
    let one = vec![row(range(0, 60), 600, &SCHEMA)];
    let single = plan_units(&[hour_bucket(true)], &one).remove(0);
    assert_eq!(targets_for(&single, &one, covered, HOUR), 0);
    // mixed schemas do not merge
    let mut mixed = minute_rows(60, 10);
    mixed[3].file_schema_hash = vec![9];
    let mixed_unit = plan_units(&[hour_bucket(true)], &mixed).remove(0);
    assert_eq!(targets_for(&mixed_unit, &mixed, covered, HOUR), 0);
    // grown unit
    let wide = vec![
        row(range(0, 60), 1, &SCHEMA),
        row(range(30, 90), 1, &SCHEMA),
    ];
    let grown = plan_units(&[hour_bucket(true)], &wide).remove(0);
    assert_eq!(grown.planned_rung, None);
    assert_eq!(targets_for(&grown, &wide, covered, HOUR), 0);
}

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

async fn factory_with_on_demand_view(lakehouse: &LakehouseContext) -> Arc<ViewFactory> {
    let blocks_view =
        Arc::new(BlocksView::new(lakehouse.default_audience()).expect("BlocksView::new"));
    let log_entries_view = LogViewMaker {}
        .make_view("global")
        .expect("log_entries global view");
    let mut factory = ViewFactory::new(vec![log_entries_view, blocks_view]);
    factory.add_view_set("log_entries".to_string(), Arc::new(LogViewMaker {}));
    let factory = Arc::new(factory);
    let def = ViewDefinition {
        view_set_name: "od_view".to_string(),
        extract_query: "SELECT date_bin('1 minute', time) as time_bin, \
                arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                count(*) as count \
         FROM log_entries \
         WHERE insert_time >= '{begin}' AND insert_time < '{end}' \
         GROUP BY time_bin"
            .to_string(),
        count_src_query: "SELECT count(*) AS count FROM log_entries \
         WHERE insert_time >= '{begin}' AND insert_time < '{end}'"
            .to_string(),
        merge_partitions_query: "SELECT time_bin, \
                arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                sum(count) as count \
         FROM {source} GROUP BY time_bin"
            .to_string(),
        update_group: None,
        options: ViewOptions {
            min_time_column: "time_bin".to_string(),
            max_time_column: "time_bin".to_string(),
            source_partition_delta: "1 day".to_string(),
            merge_partition_delta: "1 day".to_string(),
            merge_sort_order: None,
            description: None,
        },
    };
    let view = build_sql_batch_view(
        &def,
        lakehouse.runtime().clone(),
        lakehouse.lake().clone(),
        factory.clone(),
        Arc::new(NoOpSessionConfigurator),
    )
    .await
    .expect("build the on-demand view");
    let mut with_view = (*factory).clone();
    with_view.add_global_view(Arc::new(view));
    Arc::new(with_view)
}

async fn admin_session(
    lakehouse: Arc<LakehouseContext>,
    factory: Arc<ViewFactory>,
) -> datafusion::prelude::SessionContext {
    make_session_context(
        lakehouse,
        Arc::new(NullPartitionProvider {}),
        SessionRange::Snapshot,
        factory,
        Arc::new(NoOpSessionConfigurator),
        CallerContext::maintenance(),
    )
    .await
    .expect("make_session_context")
}

const FUNCTION_ARGS: &str =
    "'od_view', TIMESTAMP '2024-01-01T00:00:00Z', TIMESTAMP '2024-01-02T00:00:00Z', 3600";

#[tokio::test]
async fn materialize_partitions_rejects_an_on_demand_view() {
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = factory_with_on_demand_view(&lakehouse).await;
    let ctx = admin_session(lakehouse, factory).await;
    let batches = ctx
        .sql(&format!(
            "SELECT * FROM materialize_partitions({FUNCTION_ARGS})"
        ))
        .await
        .expect("plan")
        .collect()
        .await
        .expect("the error is reported as a log row");
    let mut messages = vec![];
    for batch in &batches {
        let column = batch
            .column_by_name("msg")
            .expect("msg column")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("msg is a string column");
        messages.extend(column.iter().flatten().map(str::to_string));
    }
    assert!(
        messages
            .iter()
            .any(|m| m.contains("od_view is an on-demand view")),
        "unexpected log rows: {messages:?}"
    );
}

#[tokio::test]
async fn regenerate_partitions_rejects_an_on_demand_view() {
    let lakehouse = make_offline_lakehouse_context().await;
    let factory = factory_with_on_demand_view(&lakehouse).await;
    let ctx = admin_session(lakehouse, factory).await;
    let err = ctx
        .sql(&format!(
            "SELECT * FROM regenerate_partitions({FUNCTION_ARGS})"
        ))
        .await
        .expect("plan")
        .collect()
        .await
        .expect_err("regenerate_partitions must reject an on-demand view");
    assert!(
        format!("{err}").contains("od_view is an on-demand view"),
        "unexpected error: {err}"
    );
}
