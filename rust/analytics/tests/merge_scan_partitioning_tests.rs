//! Offline (no live DB) regression tests for `tasks/1491_merge_scan_memory_plan.md`:
//! - `make_merge_session_context` (Design §1) forces a merge's source scan into a single
//!   sequential reader, regardless of `ScanOrdering` -- unlike the other offline merge tests in
//!   this crate, this one asserts against the wrapper itself rather than a bare config-mutating
//!   helper the test drives by hand, so it guards the wrapper's own plan shape.
//! - `QueryMerger::execute_merge_query`'s collapsed two-arm dispatch (Design §2) still runs the
//!   concatenating strategy for the default, undeclared `ScanOrdering::Unordered` ordering, and
//!   still produces the inputs concatenated in the order they were passed in.

use chrono::{DateTime, TimeDelta, Utc};
use datafusion::arrow::array::{Array, Int32Array};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::parquet::arrow::AsyncArrowWriter;
use futures::stream::StreamExt;
use micromegas_analytics::lakehouse::async_parquet_writer::AsyncParquetWriter;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::merge::{
    PartitionMerger, QueryMerger, make_merge_session_context,
};
use micromegas_analytics::lakehouse::metadata_cache::MetadataCache;
use micromegas_analytics::lakehouse::partition::Partition;
use micromegas_analytics::lakehouse::partition_cache::PartitionCache;
use micromegas_analytics::lakehouse::partitioned_table_provider::PartitionedTableProvider;
use micromegas_analytics::lakehouse::query::make_session_context;
use micromegas_analytics::lakehouse::read_scope::CallerContext;
use micromegas_analytics::lakehouse::reader_factory::ReaderFactory;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::view::ViewMetadata;
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use micromegas_analytics::time::TimeRange;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use object_store::ObjectStore;
use object_store::buffered::BufWriter;
use object_store::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

/// Builds an offline `LakehouseContext` (in-memory object store, lazily-connected -- never
/// actually queried -- Postgres pool) sufficient to build sessions and run merges without
/// touching a real database.
async fn make_offline_lakehouse_context() -> Arc<LakehouseContext> {
    let db_pool = sqlx::PgPool::connect_lazy("postgres://user:pass@127.0.0.1:1/db")
        .expect("connect_lazy should not touch the network");
    let object_store: Arc<dyn ObjectStore> = Arc::new(object_store::memory::InMemory::new());
    let blob_storage = Arc::new(BlobStorage::new(
        object_store,
        object_store::path::Path::from("lakehouse"),
    ));
    let lake = Arc::new(DataLakeConnection::new(db_pool, blob_storage));
    let runtime = Arc::new(make_runtime_env().expect("make_runtime_env"));
    Arc::new(LakehouseContext::new(lake, runtime).expect("LakehouseContext::new"))
}

fn id_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![Field::new("id", DataType::Int32, false)]))
}

fn make_reader_factory(object_store: Arc<dyn ObjectStore>) -> Arc<ReaderFactory> {
    Arc::new(ReaderFactory::new(
        object_store,
        Arc::new(MetadataCache::new(1024 * 1024)),
    ))
}

fn make_fabricated_partition(
    file_path: &str,
    file_size: i64,
    num_rows: i64,
    begin_insert: DateTime<Utc>,
) -> Partition {
    let end_insert = begin_insert + TimeDelta::seconds(1);
    Partition {
        view_metadata: ViewMetadata {
            view_set_name: Arc::new("test_view".to_owned()),
            view_instance_id: Arc::new("global".to_owned()),
            file_schema_hash: vec![0],
        },
        insert_time_range: TimeRange::new(begin_insert, end_insert),
        event_time_range: Some(TimeRange::new(begin_insert, end_insert)),
        updated: Utc::now(),
        file_path: Some(file_path.to_owned()),
        file_size,
        source_data_hash: vec![0],
        num_rows,
        sort_order: None,
        max_sort_key_time: None,
    }
}

#[tokio::test]
async fn make_merge_session_context_forces_a_single_reader_scan() {
    let lakehouse = make_offline_lakehouse_context().await;
    let schema = id_schema();
    // Well above the 10 MB `repartition_file_min_size` default, so a plain session's file-scan
    // repartitioning has room to split this file group.
    let file_size = 20 * 1024 * 1024;
    let t0 = Utc::now();
    let partitions = Arc::new(vec![
        make_fabricated_partition("a.parquet", file_size, 10, t0),
        make_fabricated_partition("b.parquet", file_size, 10, t0 + TimeDelta::hours(1)),
        make_fabricated_partition("c.parquet", file_size, 10, t0 + TimeDelta::hours(2)),
    ]);
    let reader_factory = make_reader_factory(lakehouse.lake().blob_storage.inner());
    let source: Arc<PartitionedTableProvider> = Arc::new(PartitionedTableProvider::new(
        schema,
        reader_factory,
        partitions,
    ));

    let insert_range = TimeRange::new(t0, t0 + TimeDelta::hours(3));

    let merge_ctx = make_merge_session_context(
        lakehouse.clone(),
        Arc::new(PartitionCache::empty(insert_range)),
        None,
        Arc::new(ViewFactory::new(vec![])),
        Arc::new(NoOpSessionConfigurator),
        CallerContext::maintenance(),
    )
    .await
    .expect("make_merge_session_context");
    // Neither session-builder function takes a `SessionConfig`, so `target_partitions` is set
    // after the fact -- the same way `execute_concatenated_merge` mutates optimizer options on an
    // already-built context today -- so the control assertion below isn't meaningless on a
    // low-core-count CI runner.
    merge_ctx
        .state_ref()
        .write()
        .config_mut()
        .options_mut()
        .execution
        .target_partitions = 8;
    merge_ctx
        .register_table("source", source.clone())
        .expect("register_table on merge session");
    let merge_plan = merge_ctx
        .sql("SELECT * FROM source;")
        .await
        .expect("planning over merge session")
        .create_physical_plan()
        .await
        .expect("create_physical_plan over merge session");
    assert_eq!(
        merge_plan
            .properties()
            .output_partitioning()
            .partition_count(),
        1,
        "make_merge_session_context must force the source scan into a single sequential reader"
    );

    // Control: the identical source over a plain `make_session_context`, guarding that the test
    // is meaningful -- without repartition_file_scans forced off, the scan should fan out.
    let control_ctx = make_session_context(
        lakehouse.clone(),
        Arc::new(PartitionCache::empty(insert_range)),
        None,
        Arc::new(ViewFactory::new(vec![])),
        Arc::new(NoOpSessionConfigurator),
        CallerContext::maintenance(),
    )
    .await
    .expect("make_session_context");
    control_ctx
        .state_ref()
        .write()
        .config_mut()
        .options_mut()
        .execution
        .target_partitions = 8;
    control_ctx
        .register_table("source", source)
        .expect("register_table on control session");
    let control_plan = control_ctx
        .sql("SELECT * FROM source;")
        .await
        .expect("planning over control session")
        .create_physical_plan()
        .await
        .expect("create_physical_plan over control session");
    assert!(
        control_plan
            .properties()
            .output_partitioning()
            .partition_count()
            > 1,
        "control session (plain make_session_context) should still fan the scan out to more \
         than one partition -- if not, this test would silently no-op"
    );
}

/// Writes a single-row, one-column (`id: Int32`) Parquet file directly into `object_store`
/// (the offline context's underlying, un-prefixed store -- matching what `ReaderFactory` reads
/// from), returning its exact byte size so the fabricated `Partition::file_size` matches: an
/// inflated size makes `ParquetOpener`'s footer read run past the end of the object.
async fn write_single_row_parquet(
    object_store: Arc<dyn ObjectStore>,
    file_path: &str,
    schema: Arc<Schema>,
    id_value: i32,
) -> i64 {
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(Int32Array::from(vec![id_value]))],
    )
    .expect("building one-row batch");
    let byte_counter = Arc::new(AtomicI64::new(0));
    let buf_writer = BufWriter::new(object_store, Path::from(file_path));
    let parquet_writer = AsyncParquetWriter::new(buf_writer, byte_counter.clone());
    let mut writer =
        AsyncArrowWriter::try_new(parquet_writer, schema, None).expect("AsyncArrowWriter::try_new");
    writer.write(&batch).await.expect("writer.write");
    writer.close().await.expect("writer.close");
    byte_counter.load(Ordering::Relaxed)
}

#[tokio::test]
async fn execute_merge_query_concatenates_partitions_in_the_order_passed_in() {
    let lakehouse = make_offline_lakehouse_context().await;
    let schema = id_schema();
    let object_store = lakehouse.lake().blob_storage.inner();

    let t0 = Utc::now();
    let first_size =
        write_single_row_parquet(object_store.clone(), "first.parquet", schema.clone(), 0).await;
    let second_size =
        write_single_row_parquet(object_store.clone(), "second.parquet", schema.clone(), 1).await;

    // Passed to execute_merge_query with the *later*-insert-time partition first: production
    // begin_insert_time ordering is applied by create_merged_partition's caller-side sort
    // (merge.rs), not by anything under test here, so if this method accidentally re-sorted by
    // insert time the observed row order would flip and this test would catch it.
    let part_first =
        make_fabricated_partition("first.parquet", first_size, 1, t0 + TimeDelta::hours(1));
    let part_second = make_fabricated_partition("second.parquet", second_size, 1, t0);

    let insert_range = TimeRange::new(t0, t0 + TimeDelta::hours(2));
    let merger = QueryMerger::new(
        Arc::new(ViewFactory::new(vec![])),
        Arc::new(NoOpSessionConfigurator),
        schema,
        Arc::new(String::from("SELECT * FROM source;")),
    );
    let result = merger
        .execute_merge_query(
            lakehouse,
            Arc::new(vec![part_first, part_second]),
            Arc::new(PartitionCache::empty(insert_range)),
            insert_range,
        )
        .await
        .expect("execute_merge_query should succeed");

    let mut stream = result.stream;
    let mut ids = Vec::new();
    while let Some(rb) = stream.next().await {
        let rb = rb.expect("record batch");
        let col = rb
            .column(0)
            .as_any()
            .downcast_ref::<Int32Array>()
            .expect("id column is Int32Array");
        ids.extend(col.iter().map(|v| v.expect("non-null id")));
    }
    assert_eq!(
        ids,
        vec![0, 1],
        "rows must come out concatenated in the order partitions were passed to \
         execute_merge_query -- begin_insert_time ordering is the caller's job, not this method's"
    );
}
