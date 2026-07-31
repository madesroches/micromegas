//! Offline (no live DB) regression tests for `SqlBatchView::with_merge_sort_order`
//! (`tasks/1392_kway_merge_sorted_partitions_plan.md` Design §3, Testing Strategy items 4-5):
//! - the dual-merger selection and `get_merged_partition_sort_order` gate across the input matrix
//!   (all certified / one uncertified / all empty / mixed)
//! - `register_table` keeps registering `merge_partitions_query` verbatim: the user-facing view's
//!   *logical* plan carries no `Sort` node above the merge query, even though the same view's
//!   `merge_partitions` (through its `ordered_merger`) does apply one internally
//! - the user-query-path plan shape (Design §3's memory reasoning and the "No ceiling on k"
//!   trade-off both depend on it): no `SortExec`, `ordering_mode=Sorted`, and the order-preserving
//!   `RepartitionExec(Hash(...), preserve_order=true)` splitting k declared per-file groups into
//!   `target_partitions` byte-range groups once `target_partitions` is pinned above k

use chrono::{TimeDelta, Utc};
use datafusion::arrow::array::{Array, RecordBatch, StringArray};
use datafusion::physical_plan::displayable;
use datafusion::prelude::{SessionConfig, SessionContext};
use futures::TryStreamExt;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::materialized_view::MaterializedView;
use micromegas_analytics::lakehouse::metadata_cache::MetadataCache;
use micromegas_analytics::lakehouse::partition::Partition;
use micromegas_analytics::lakehouse::partition_cache::{
    NullPartitionProvider, PartitionCache, QueryPartitionProvider,
};
use micromegas_analytics::lakehouse::partitioned_execution_plan::ScanOrdering;
use micromegas_analytics::lakehouse::partitioned_table_provider::PartitionedTableProvider;
use micromegas_analytics::lakehouse::reader_factory::ReaderFactory;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::sql_batch_view::SqlBatchView;
use micromegas_analytics::lakehouse::view::{ScanSortColumn, View, ViewMetadata};
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use micromegas_analytics::time::TimeRange;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use std::sync::Arc;

/// Builds an offline `LakehouseContext` (in-memory object store, lazily-connected -- never
/// actually queried -- Postgres pool), the same pattern `blocks_view_merge_ordering_tests.rs` uses.
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
    Arc::new(LakehouseContext::new(lake, runtime))
}

/// A fabricated `SqlBatchView` whose extract query is entirely self-contained (no `FROM` table),
/// so it needs neither a real database nor a registered view factory to resolve its schema.
/// `merge_query` is the only thing that varies across this file's fixtures.
async fn make_test_view_with_merge_query(
    lakehouse: &LakehouseContext,
    merge_query: &str,
) -> SqlBatchView {
    let count_src_query = Arc::new(String::from("SELECT 0::BIGINT as count"));
    let extract_query = Arc::new(String::from(
        "SELECT 'n' AS name, TIMESTAMP '1970-01-01 00:00:00' AS time_bin, 1 AS measure \
         ORDER BY name, time_bin",
    ));
    let merge_query = Arc::new(merge_query.to_owned());
    let time_column = Arc::new(String::from("time_bin"));
    SqlBatchView::new(
        lakehouse.runtime().clone(),
        Arc::new("test_metrics".to_owned()),
        time_column.clone(),
        time_column,
        count_src_query,
        extract_query,
        merge_query,
        lakehouse.lake().clone(),
        Arc::new(ViewFactory::new(vec![])),
        Arc::new(NoOpSessionConfigurator),
        None,
        TimeDelta::days(1),
        TimeDelta::days(1),
        None,
    )
    .await
    .expect("SqlBatchView::new")
    .with_merge_sort_order(vec![
        Arc::new("name".to_owned()),
        Arc::new("time_bin".to_owned()),
    ])
    .expect("with_merge_sort_order")
}

async fn make_test_view(lakehouse: &LakehouseContext) -> SqlBatchView {
    make_test_view_with_merge_query(
        lakehouse,
        "SELECT name, time_bin, sum(measure) AS measure FROM {source} GROUP BY name, time_bin",
    )
    .await
}

/// A merge query whose rows come from a `VALUES` literal already in *descending* declared order:
/// it needs no parquet file to run, and its output order alone reveals which merger ran, since only
/// the ordered merger applies the declared ascending sort on top of the author's query.
const DESCENDING_VALUES_MERGE_QUERY: &str = "SELECT * FROM (VALUES \
     ('b', TIMESTAMP '1970-01-01 00:00:01', 1), \
     ('a', TIMESTAMP '1970-01-01 00:00:00', 2)) AS t(name, time_bin, measure) \
     ORDER BY name DESC, time_bin DESC";

fn certifying_sort_order() -> Option<Vec<String>> {
    Some(vec!["name".to_owned(), "time_bin".to_owned()])
}

/// Merges `partitions` through `view` and returns the `name` column of the result, in output order.
async fn merged_names(
    lakehouse: Arc<LakehouseContext>,
    view: &SqlBatchView,
    partitions: Vec<Partition>,
) -> Vec<String> {
    let insert_range = TimeRange::new(Utc::now(), Utc::now() + TimeDelta::hours(1));
    let result = view
        .merge_partitions(
            lakehouse,
            Arc::new(partitions),
            Arc::new(PartitionCache::empty(insert_range)),
            insert_range,
        )
        .await
        .expect("merge_partitions should succeed");
    let batches: Vec<RecordBatch> = result
        .stream
        .try_collect()
        .await
        .expect("collecting the merge stream");
    batches
        .iter()
        .flat_map(|batch| {
            let column = batch.column_by_name("name").expect("name column");
            let names = column
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("name should be a StringArray");
            (0..names.len())
                .map(|i| names.value(i).to_owned())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Whether `merge_partitions` ran the ordered merger for `partitions`, observed behaviorally on a
/// probe view carrying the same sort-order declaration as this file's other fixtures: only the
/// ordered merger sorts, so ascending output means it ran and descending output means it did not.
/// This is the only externally visible difference between the two mergers.
async fn ordered_merger_ran(lakehouse: Arc<LakehouseContext>, partitions: Vec<Partition>) -> bool {
    let view = make_test_view_with_merge_query(&lakehouse, DESCENDING_VALUES_MERGE_QUERY).await;
    let names = merged_names(lakehouse, &view, partitions).await;
    match names.join(",").as_str() {
        "a,b" => true,
        "b,a" => false,
        other => panic!("unexpected merge output rows [{other}]"),
    }
}

fn make_partition(
    file_path: Option<&str>,
    sort_order: Option<Vec<String>>,
    index: i64,
) -> Partition {
    let t0 = Utc::now() + TimeDelta::seconds(index * 10);
    let is_empty = file_path.is_none();
    Partition {
        view_metadata: ViewMetadata {
            view_set_name: Arc::new("test_metrics".to_owned()),
            view_instance_id: Arc::new("global".to_owned()),
            file_schema_hash: vec![0],
        },
        insert_time_range: TimeRange::new(t0, t0 + TimeDelta::seconds(10)),
        event_time_range: if is_empty {
            None
        } else {
            Some(TimeRange::new(t0, t0 + TimeDelta::seconds(10)))
        },
        updated: t0,
        file_path: file_path.map(|s| s.to_owned()),
        file_size: if is_empty { 0 } else { 1024 },
        source_data_hash: vec![0],
        num_rows: if is_empty { 0 } else { 10 },
        sort_order,
    }
}

#[tokio::test]
async fn all_certified_inputs_use_the_ordered_merger_and_record_sort_order() {
    let lakehouse = make_offline_lakehouse_context().await;
    let view = make_test_view(&lakehouse).await;
    let partitions = vec![
        make_partition(Some("a.parquet"), certifying_sort_order(), 0),
        make_partition(Some("b.parquet"), certifying_sort_order(), 1),
    ];
    assert_eq!(
        view.get_merged_partition_sort_order(&partitions),
        certifying_sort_order(),
        "all-certified inputs should record the declared sort_order"
    );
    assert!(
        ordered_merger_ran(lakehouse.clone(), partitions.clone()).await,
        "all-certified inputs should take the ordered (PerFile) merger"
    );

    let insert_range = TimeRange::new(Utc::now(), Utc::now() + TimeDelta::hours(1));
    let result = view
        .merge_partitions(
            lakehouse,
            Arc::new(partitions),
            Arc::new(PartitionCache::empty(insert_range)),
            insert_range,
        )
        .await
        .expect("merge_partitions should succeed");
    assert!(result.ordering_honored);
}

#[tokio::test]
async fn one_uncertified_input_falls_back_to_the_plain_merger() {
    let lakehouse = make_offline_lakehouse_context().await;
    let view = make_test_view(&lakehouse).await;
    let partitions = vec![
        make_partition(Some("a.parquet"), certifying_sort_order(), 0),
        // Never merged/regenerated under the new declaration -- no sort_order recorded.
        make_partition(Some("b.parquet"), None, 1),
    ];
    assert_eq!(
        view.get_merged_partition_sort_order(&partitions),
        None,
        "a single uncertified input must not record a false sort_order guarantee"
    );
    assert!(
        !ordered_merger_ran(lakehouse.clone(), partitions.clone()).await,
        "a single uncertified input must fall back to the plain merger"
    );

    let insert_range = TimeRange::new(Utc::now(), Utc::now() + TimeDelta::hours(1));
    let result = view
        .merge_partitions(
            lakehouse,
            Arc::new(partitions),
            Arc::new(PartitionCache::empty(insert_range)),
            insert_range,
        )
        .await
        .expect("merge_partitions should succeed");
    assert!(
        result.ordering_honored,
        "the plain merger reports ordering_honored: true"
    );
}

#[tokio::test]
async fn all_empty_inputs_record_sort_order_vacuously_but_use_the_plain_merger() {
    let lakehouse = make_offline_lakehouse_context().await;
    let view = make_test_view(&lakehouse).await;
    let partitions = vec![make_partition(None, None, 0), make_partition(None, None, 1)];
    assert_eq!(
        view.get_merged_partition_sort_order(&partitions),
        certifying_sort_order(),
        "an all-empty merge certifies vacuously, matching the blocks_view precedent"
    );
    assert!(
        !ordered_merger_ran(lakehouse.clone(), partitions.clone()).await,
        "an all-empty merge must still use the plain merger -- an EmptyExec's SortExec is never \
         elided, which would trip the memory-regression warning on every quiet-day retry"
    );

    let insert_range = TimeRange::new(Utc::now(), Utc::now() + TimeDelta::hours(1));
    let result = view
        .merge_partitions(
            lakehouse,
            Arc::new(partitions),
            Arc::new(PartitionCache::empty(insert_range)),
            insert_range,
        )
        .await
        .expect("merge_partitions should succeed");
    assert!(result.ordering_honored);
}

#[tokio::test]
async fn mixed_certified_and_empty_inputs_use_the_ordered_merger() {
    let lakehouse = make_offline_lakehouse_context().await;
    let view = make_test_view(&lakehouse).await;
    let partitions = vec![
        make_partition(Some("a.parquet"), certifying_sort_order(), 0),
        make_partition(None, None, 1),
    ];
    assert_eq!(
        view.get_merged_partition_sort_order(&partitions),
        certifying_sort_order()
    );
    assert!(
        ordered_merger_ran(lakehouse.clone(), partitions.clone()).await,
        "a certifying non-empty input alongside an empty one should still take the ordered path"
    );

    let insert_range = TimeRange::new(Utc::now(), Utc::now() + TimeDelta::hours(1));
    let result = view
        .merge_partitions(
            lakehouse,
            Arc::new(partitions),
            Arc::new(PartitionCache::empty(insert_range)),
            insert_range,
        )
        .await
        .expect("merge_partitions should succeed");
    assert!(result.ordering_honored);
}

#[tokio::test]
async fn register_table_never_carries_the_merge_query_sort_node() {
    let lakehouse = make_offline_lakehouse_context().await;
    let view: Arc<dyn View> = Arc::new(make_test_view(&lakehouse).await);
    let reader_factory = Arc::new(ReaderFactory::new(
        Arc::new(object_store::memory::InMemory::new()),
        Arc::new(MetadataCache::new(1024 * 1024)),
    ));
    let part_provider: Arc<dyn QueryPartitionProvider> = Arc::new(NullPartitionProvider {});
    let materialized =
        MaterializedView::new(lakehouse, reader_factory, view.clone(), part_provider, None);

    let ctx = SessionContext::new();
    view.register_table(&ctx, materialized)
        .await
        .expect("register_table");
    let registered = ctx.table("test_metrics").await.expect("ctx.table");
    let logical_plan_str = format!("{}", registered.logical_plan().display_indent());
    assert!(
        !logical_plan_str.contains("Sort:"),
        "register_table's registered view must not carry a Sort node above the merge query -- the \
         sort is only ever applied inside QueryMerger::execute_merge_query's ordered merge path, \
         got:\n{logical_plan_str}"
    );
}

#[tokio::test]
async fn user_query_path_stays_streaming_with_target_partitions_pinned_above_k() {
    // Registers a SqlBatchView-shaped table (the same PartitionedTableProvider a real
    // get_scan_output_ordering() -> PerFile view would produce) directly in a default (user)
    // session -- not a merge-query session with QueryMerger's optimizer overrides -- pinning
    // target_partitions explicitly above k so the assertion doesn't silently no-op on a
    // low-core-count CI runner (Design §3 / Testing Strategy item 5).
    let k: usize = 3;
    let columns = vec![
        ScanSortColumn {
            column: Arc::new("name".to_owned()),
            descending: false,
        },
        ScanSortColumn {
            column: Arc::new("time_bin".to_owned()),
            descending: false,
        },
    ];
    let schema = Arc::new(datafusion::arrow::datatypes::Schema::new(vec![
        datafusion::arrow::datatypes::Field::new(
            "name",
            datafusion::arrow::datatypes::DataType::Utf8,
            false,
        ),
        datafusion::arrow::datatypes::Field::new(
            "time_bin",
            datafusion::arrow::datatypes::DataType::Timestamp(
                datafusion::arrow::datatypes::TimeUnit::Nanosecond,
                Some("+00:00".into()),
            ),
            false,
        ),
        datafusion::arrow::datatypes::Field::new(
            "measure",
            datafusion::arrow::datatypes::DataType::Int64,
            false,
        ),
    ]));
    let reader_factory = Arc::new(ReaderFactory::new(
        Arc::new(object_store::memory::InMemory::new()),
        Arc::new(MetadataCache::new(1024 * 1024)),
    ));
    let partitions: Vec<Partition> = (0..k as i64)
        .map(|i| {
            make_partition(
                Some(&format!("part_{i}.parquet")),
                Some(vec!["name".to_owned(), "time_bin".to_owned()]),
                i,
            )
        })
        .collect();
    let provider = PartitionedTableProvider::with_scan_ordering(
        schema,
        reader_factory,
        Arc::new(partitions),
        ScanOrdering::PerFile { columns },
    );

    let config = SessionConfig::new().with_target_partitions(8);
    let ctx = SessionContext::new_with_config(config);
    ctx.register_table("source", Arc::new(provider))
        .expect("register_table");
    let df = ctx
        .sql("SELECT name, time_bin, sum(measure) as measure FROM source GROUP BY name, time_bin")
        .await
        .expect("sql");
    let plan = df
        .create_physical_plan()
        .await
        .expect("create_physical_plan");
    let plan_str = displayable(plan.as_ref()).indent(true).to_string();

    assert!(
        !plan_str.contains("SortExec"),
        "the user-query path must stay streaming with no blocking Sort, got:\n{plan_str}"
    );
    assert!(
        plan_str.contains("ordering_mode=Sorted"),
        "expected order-aware aggregation from the declared per-file scan ordering, got:\n{plan_str}"
    );
    assert!(
        plan_str.contains("partitioning=Hash(") && plan_str.contains("preserve_order=true"),
        "expected FileGroupPartitioner to split the {k} declared per-file groups into \
         target_partitions byte-range groups via an order-preserving hash repartition, \
         got:\n{plan_str}"
    );
}

#[tokio::test]
async fn build_side_enrichment_join_reports_ordering_not_honored_without_erroring() {
    // Plan Testing Strategy item 3 / Design §5 correction 3: a realistic authoring mistake is an
    // enrichment join phrased with the ordered aggregate on the *build* side of a `LEFT JOIN`
    // (`CollectLeft` buffers its left input and inherits ordering from the right/probe side, so
    // the naturally-phrased `(<ordered agg>) a LEFT JOIN dim d` reinstates a blocking SortExec --
    // see `ordered_aggregation_spike_tests.rs`'s
    // `enrichment_join_with_the_ordered_side_on_the_build_side_reinstates_a_blocking_sort`). Even
    // under the `PerFile` merge branch's mandatory trailing `DataFrame::sort` (merge.rs:183-188),
    // the plan still carries a `SortExec` -- this must warn (check 3), not fail the merge. A false
    // `ordering_honored` is itself proof the ordered merger ran: the plain one always reports true.
    let lakehouse = make_offline_lakehouse_context().await;
    let view = make_test_view_with_merge_query(
        &lakehouse,
        "SELECT a.name, a.time_bin, a.measure, d.label FROM \
         (SELECT name, time_bin, sum(measure) AS measure FROM {source} GROUP BY name, time_bin) a \
         LEFT JOIN (VALUES ('n', 'lbl')) AS d(name, label) ON a.name = d.name",
    )
    .await;
    let partitions = vec![
        make_partition(Some("a.parquet"), certifying_sort_order(), 0),
        make_partition(Some("b.parquet"), certifying_sort_order(), 1),
    ];
    let insert_range = TimeRange::new(Utc::now(), Utc::now() + TimeDelta::hours(1));
    let result = view
        .merge_partitions(
            lakehouse,
            Arc::new(partitions),
            Arc::new(PartitionCache::empty(insert_range)),
            insert_range,
        )
        .await
        .expect("a defeated elision must not fail the merge (fail-open)");
    assert!(
        !result.ordering_honored,
        "expected the ordered aggregate sitting on the join's build side to reinstate a blocking \
         SortExec, so ordering_honored should be false"
    );
}

#[tokio::test]
async fn declared_ascending_sort_order_overrides_a_contradictory_author_order_by_desc() {
    // Testing Strategy item 3: the merge query's own (contradictory) `ORDER BY ... DESC` must not
    // win -- `execute_per_file_merge`'s unconditional `DataFrame::sort` over the declared ascending
    // columns (merge.rs:183-188) is applied on top of the author's query, so the declared order
    // always wins. Two out-of-order rows ('b' before 'a', matching the author's DESC clause) make
    // the override observable: an ascending result can only come from the declared sort order, not
    // from the query's own `ORDER BY`.
    let lakehouse = make_offline_lakehouse_context().await;
    let view = make_test_view_with_merge_query(&lakehouse, DESCENDING_VALUES_MERGE_QUERY).await;
    let partitions = vec![
        make_partition(Some("a.parquet"), certifying_sort_order(), 0),
        make_partition(Some("b.parquet"), certifying_sort_order(), 1),
    ];
    assert_eq!(
        view.get_merged_partition_sort_order(&partitions),
        certifying_sort_order(),
        "the declared ascending sort order must still be recorded despite the author's DESC query"
    );

    let names = merged_names(lakehouse, &view, partitions).await;
    assert_eq!(
        names,
        vec!["a".to_owned(), "b".to_owned()],
        "expected the declared ascending (name, time_bin) order to win over the author's \
         `ORDER BY ... DESC`"
    );
}
