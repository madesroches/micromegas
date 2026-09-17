//! Offline (no live DB) tests for `ViewRegistry`, driven by a fake `ViewDefinitionStore` over
//! canned rows -- no `sqlx::Transaction` or live Postgres needed.

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use micromegas_analytics::lakehouse::blocks_view::BlocksView;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::log_view::LogViewMaker;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::view_definition::ViewOptions;
use micromegas_analytics::lakehouse::view_definition_store::{
    ViewDefinitionRow, ViewDefinitionStore,
};
use micromegas_analytics::lakehouse::view_factory::{ViewFactory, ViewMaker};
use micromegas_analytics::lakehouse::view_registry::ViewRegistry;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use std::sync::{Arc, Mutex};

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

fn make_base_factory(lakehouse: &LakehouseContext) -> Arc<ViewFactory> {
    let blocks_view =
        Arc::new(BlocksView::new(lakehouse.default_audience()).expect("BlocksView::new"));
    let log_view_maker = LogViewMaker {};
    let log_entries_view = log_view_maker
        .make_view("global")
        .expect("log_entries global view");
    let mut factory = ViewFactory::new(vec![log_entries_view, blocks_view]);
    factory.add_view_set("log_entries".to_string(), Arc::new(LogViewMaker {}));
    Arc::new(factory)
}

#[derive(Debug, Default)]
struct FakeStore {
    rows: Mutex<Vec<ViewDefinitionRow>>,
}

impl FakeStore {
    fn new(rows: Vec<ViewDefinitionRow>) -> Self {
        Self {
            rows: Mutex::new(rows),
        }
    }

    fn set_rows(&self, rows: Vec<ViewDefinitionRow>) {
        *self.rows.lock().expect("lock") = rows;
    }
}

#[async_trait]
impl ViewDefinitionStore for FakeStore {
    async fn list(&self) -> anyhow::Result<Vec<ViewDefinitionRow>> {
        // Matches `PgViewDefinitionStore::list`'s `ORDER BY update_group, view_set_name` --
        // `build_factory` relies on the store handing back rows already in that order.
        let mut rows = self.rows.lock().expect("lock").clone();
        rows.sort_by(|a, b| {
            a.update_group
                .cmp(&b.update_group)
                .then_with(|| a.view_set_name.cmp(&b.view_set_name))
        });
        Ok(rows)
    }
}

const VALID_COUNT_SRC: &str = "SELECT sum(nb_objects) as count FROM blocks \
     WHERE insert_time >= '{begin}' AND insert_time < '{end}'";

fn row(
    name: &str,
    extract_query: &str,
    merge_partitions_query: &str,
    update_group: i32,
    updated_at: DateTime<Utc>,
) -> ViewDefinitionRow {
    let options = ViewOptions {
        min_time_column: "time_bin".to_string(),
        max_time_column: "time_bin".to_string(),
        source_partition_delta: "1 day".to_string(),
        merge_partition_delta: "1 day".to_string(),
        merge_sort_order: None,
    };
    ViewDefinitionRow {
        view_set_name: name.to_string(),
        definition_sql: format!("CREATE MATERIALIZED VIEW {name} WITH (...)"),
        extract_query: extract_query.to_string(),
        count_src_query: VALID_COUNT_SRC.to_string(),
        merge_partitions_query: merge_partitions_query.to_string(),
        update_group,
        view_options: serde_json::to_string(&options).expect("serialize ViewOptions"),
        created_at: updated_at,
        updated_at,
        updated_by: Some("test".to_string()),
    }
}

/// `a`: reads `log_entries` directly (update_group 2500), carries `audience`, `count`, and an
/// extra `level` column so a dependent (`b`) can read it.
fn row_a(updated_at: DateTime<Utc>, keep_level: bool) -> ViewDefinitionRow {
    let level_select = if keep_level {
        ", CAST(1 AS INT) as level"
    } else {
        ""
    };
    let level_merge = if keep_level {
        ", max(level) as level"
    } else {
        ""
    };
    row(
        "a",
        &format!(
            "SELECT date_bin('1 minute', time) as time_bin, \
                    arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                    count(*) as count{level_select} \
             FROM log_entries \
             WHERE insert_time >= '{{begin}}' AND insert_time < '{{end}}' \
             GROUP BY time_bin"
        ),
        &format!(
            "SELECT time_bin, arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                    sum(count) as count{level_merge} \
             FROM {{source}} GROUP BY time_bin"
        ),
        2500,
        updated_at,
    )
}

/// `b`: reads `a` (update_group 2600), including its `level` column.
fn row_b(updated_at: DateTime<Utc>) -> ViewDefinitionRow {
    row(
        "b",
        "SELECT time_bin, audience, count, level FROM a",
        "SELECT time_bin, arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                sum(count) as count, max(level) as level \
         FROM {source} GROUP BY time_bin",
        2600,
        updated_at,
    )
}

/// `c`: independent of `a`/`b`, reads `log_entries` directly at its own group.
fn row_c(updated_at: DateTime<Utc>) -> ViewDefinitionRow {
    row(
        "c",
        "SELECT date_bin('1 minute', time) as time_bin, \
                arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                count(*) as count \
         FROM log_entries \
         WHERE insert_time >= '{begin}' AND insert_time < '{end}' \
         GROUP BY time_bin",
        "SELECT time_bin, arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience, \
                sum(count) as count \
         FROM {source} GROUP BY time_bin",
        2700,
        updated_at,
    )
}

/// A definition whose extract query can never resolve.
fn row_broken(updated_at: DateTime<Utc>) -> ViewDefinitionRow {
    row(
        "broken",
        "SELECT * FROM this_table_does_not_exist",
        "SELECT * FROM {source}",
        2900,
        updated_at,
    )
}

fn make_registry(lakehouse: &LakehouseContext, store: Arc<FakeStore>) -> ViewRegistry {
    ViewRegistry::new(
        make_base_factory(lakehouse),
        store,
        lakehouse.runtime().clone(),
        lakehouse.lake().clone(),
        Arc::new(NoOpSessionConfigurator),
    )
}

#[tokio::test]
async fn current_returns_base_factory_before_first_reload() {
    let lakehouse = make_offline_lakehouse_context().await;
    let base_count = make_base_factory(&lakehouse).get_global_views().len();
    let store = Arc::new(FakeStore::new(vec![row_a(Utc::now(), true)]));
    let registry = make_registry(&lakehouse, store);
    assert_eq!(registry.current().get_global_views().len(), base_count);
}

#[tokio::test]
async fn reload_builds_in_update_group_order_and_a_higher_group_can_read_a_lower_one() {
    let lakehouse = make_offline_lakehouse_context().await;
    let now = Utc::now();
    let store = Arc::new(FakeStore::new(vec![row_b(now), row_a(now, true)]));
    let registry = make_registry(&lakehouse, store);
    registry.reload().await.expect("reload");
    assert!(
        registry.failed_view_sets().is_empty(),
        "{:?}",
        registry.failed_view_sets()
    );
    let factory = registry.current();
    assert!(factory.get_global_view("a").is_some());
    assert!(
        factory.get_global_view("b").is_some(),
        "b, whose extract_query reads a, must build successfully when a is folded in first \
         (rows are always listed in (update_group, view_set_name) order)"
    );
}

#[tokio::test]
async fn a_definition_that_fails_to_build_is_skipped_and_reported() {
    let lakehouse = make_offline_lakehouse_context().await;
    let now = Utc::now();
    let store = Arc::new(FakeStore::new(vec![row_a(now, true), row_broken(now)]));
    let registry = make_registry(&lakehouse, store);
    registry.reload().await.expect("reload");
    assert_eq!(registry.failed_view_sets(), vec!["broken".to_string()]);
    let factory = registry.current();
    assert!(factory.get_global_view("a").is_some());
    assert!(factory.get_global_view("broken").is_none());
}

#[tokio::test]
async fn unchanged_row_set_short_circuits_without_rebuilding() {
    let lakehouse = make_offline_lakehouse_context().await;
    let now = Utc::now();
    let store = Arc::new(FakeStore::new(vec![row_a(now, true)]));
    let registry = make_registry(&lakehouse, store);
    registry.reload().await.expect("first reload");
    let first = registry.current();
    registry.reload().await.expect("second reload");
    let second = registry.current();
    assert!(
        Arc::ptr_eq(&first, &second),
        "an unchanged (name, updated_at) set must short-circuit rather than rebuild the factory"
    );
}

#[tokio::test]
async fn a_dropped_definition_disappears_from_the_swapped_factory() {
    let lakehouse = make_offline_lakehouse_context().await;
    let now = Utc::now();
    let store = Arc::new(FakeStore::new(vec![row_a(now, true)]));
    let registry = make_registry(&lakehouse, store.clone());
    registry.reload().await.expect("first reload");
    assert!(registry.current().get_global_view("a").is_some());

    store.set_rows(vec![]);
    registry.reload().await.expect("second reload");
    assert!(registry.current().get_global_view("a").is_none());
}

#[tokio::test]
async fn dependent_protection_refuses_to_drop_a_definition_another_one_reads() {
    let lakehouse = make_offline_lakehouse_context().await;
    let now = Utc::now();
    let registry = make_registry(&lakehouse, Arc::new(FakeStore::default()));
    let pre_rows = vec![row_a(now, true), row_b(now)];
    let post_rows = vec![row_b(now)];
    let err = registry
        .check_dependents_survive(&pre_rows, &post_rows)
        .await
        .expect_err("dropping a must be refused since b reads it");
    assert!(format!("{err:#}").contains('b'));
}

#[tokio::test]
async fn dependent_protection_accepts_a_replacement_that_still_projects_the_read_columns() {
    let lakehouse = make_offline_lakehouse_context().await;
    let now = Utc::now();
    let later = now + TimeDelta::seconds(1);
    let registry = make_registry(&lakehouse, Arc::new(FakeStore::default()));
    let pre_rows = vec![row_a(now, true), row_b(now)];
    let post_rows = vec![row_a(later, true), row_b(now)];
    registry
        .check_dependents_survive(&pre_rows, &post_rows)
        .await
        .expect("a replacement that still projects `level` must be accepted");
}

#[tokio::test]
async fn dependent_protection_refuses_a_replacement_that_drops_a_read_column() {
    let lakehouse = make_offline_lakehouse_context().await;
    let now = Utc::now();
    let later = now + TimeDelta::seconds(1);
    let registry = make_registry(&lakehouse, Arc::new(FakeStore::default()));
    let pre_rows = vec![row_a(now, true), row_b(now)];
    let post_rows = vec![row_a(later, false), row_b(now)];
    let err = registry
        .check_dependents_survive(&pre_rows, &post_rows)
        .await
        .expect_err("dropping `level` from a must be refused since b reads it");
    assert!(format!("{err:#}").contains('b'));
}

#[tokio::test]
async fn dependent_protection_accepts_dropping_a_definition_nothing_reads() {
    let lakehouse = make_offline_lakehouse_context().await;
    let now = Utc::now();
    let registry = make_registry(&lakehouse, Arc::new(FakeStore::default()));
    let pre_rows = vec![row_a(now, true), row_c(now)];
    let post_rows = vec![row_a(now, true)];
    registry
        .check_dependents_survive(&pre_rows, &post_rows)
        .await
        .expect("dropping c, which nothing reads, must be accepted");
}

#[tokio::test]
async fn dependent_protection_ignores_a_definition_already_failing_to_build() {
    let lakehouse = make_offline_lakehouse_context().await;
    let now = Utc::now();
    let registry = make_registry(&lakehouse, Arc::new(FakeStore::default()));
    let pre_rows = vec![row_a(now, true), row_c(now), row_broken(now)];
    let post_rows = vec![row_a(now, true), row_broken(now)];
    registry
        .check_dependents_survive(&pre_rows, &post_rows)
        .await
        .expect(
            "an already-broken definition must not by itself block an unrelated DROP of \
             something else",
        );
}
