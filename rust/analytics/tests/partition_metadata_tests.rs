use bytes::Bytes;
use datafusion::arrow::array::Int32Array;
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::parquet::arrow::ArrowWriter;
use datafusion::parquet::file::metadata::ParquetMetaData;
use micromegas_analytics::lakehouse::metadata_cache::MetadataCache;
use micromegas_analytics::lakehouse::partition_metadata::load_partition_metadata;
use object_store::ObjectStore;
use object_store::ObjectStoreExt;
use object_store::memory::InMemory;
use object_store::path::Path;
use std::sync::Arc;

/// Writes a small parquet file in memory, returning its bytes together with the
/// `ParquetMetaData` the writer itself produced while writing (the ground truth to compare
/// against, since `parse_parquet_metadata` only understands the extracted `FileMetaData` thrift
/// payload, not a whole parquet file's bytes).
fn write_test_parquet() -> (Bytes, ParquetMetaData) {
    let schema = Arc::new(Schema::new(vec![Field::new("a", DataType::Int32, false)]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(Int32Array::from(vec![1, 2, 3, 4, 5]))],
    )
    .expect("building record batch");
    let mut buffer = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut buffer, schema, None).expect("creating ArrowWriter");
    writer.write(&batch).expect("writing record batch");
    let metadata = writer.close().expect("closing ArrowWriter");
    (Bytes::from(buffer), metadata)
}

#[tokio::test]
async fn test_load_partition_metadata_matches_direct_parse() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let path = Path::from("test/footer.parquet");
    let (data, from_direct_parse) = write_test_parquet();
    store
        .put(&path, data.clone().into())
        .await
        .expect("put should succeed");

    let from_footer = load_partition_metadata(&store, &path, data.len() as u64, None)
        .await
        .expect("load_partition_metadata should succeed");

    // These fields are invariant under the column-index strip that
    // load_partition_metadata applies, so no stripped reference is needed.
    assert_eq!(
        from_footer.file_metadata().schema(),
        from_direct_parse.file_metadata().schema()
    );
    assert_eq!(
        from_footer.num_row_groups(),
        from_direct_parse.num_row_groups()
    );
    assert_eq!(
        from_footer.file_metadata().num_rows(),
        from_direct_parse.file_metadata().num_rows()
    );
    for i in 0..from_footer.num_row_groups() {
        assert_eq!(
            from_footer.row_group(i).num_rows(),
            from_direct_parse.row_group(i).num_rows()
        );
        assert_eq!(
            from_footer.row_group(i).num_columns(),
            from_direct_parse.row_group(i).num_columns()
        );
    }
}

#[tokio::test]
async fn test_load_partition_metadata_with_metadata_cache() {
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let path = Path::from("test/footer_with_cache.parquet");
    let (data, _from_direct_parse) = write_test_parquet();
    store
        .put(&path, data.clone().into())
        .await
        .expect("put should succeed");

    let metadata_cache = MetadataCache::new(1024 * 1024);

    // First call: metadata cache miss, should parse the footer and backfill the cache.
    let first = load_partition_metadata(&store, &path, data.len() as u64, Some(&metadata_cache))
        .await
        .expect("first load_partition_metadata should succeed");

    // Run pending tasks to ensure stats are up-to-date
    metadata_cache.run_pending_tasks().await;

    let (entry_count, weighted_size_bytes) = metadata_cache.stats();
    assert_eq!(
        entry_count, 1,
        "metadata cache should have one entry after a miss+backfill"
    );
    assert!(
        weighted_size_bytes > 0,
        "metadata cache weight should reflect footer bytes read, got {weighted_size_bytes}"
    );

    // Second call: metadata cache hit, should return equivalent metadata without needing a
    // fresh footer read.
    let second = load_partition_metadata(&store, &path, data.len() as u64, Some(&metadata_cache))
        .await
        .expect("second load_partition_metadata should succeed");

    assert_eq!(
        first.file_metadata().schema(),
        second.file_metadata().schema()
    );
    assert_eq!(first.num_row_groups(), second.num_row_groups());
    assert_eq!(
        first.file_metadata().num_rows(),
        second.file_metadata().num_rows()
    );

    // Cache should still report exactly one entry: the second call was a hit, not a new insert.
    metadata_cache.run_pending_tasks().await;
    let (entry_count_after, _) = metadata_cache.stats();
    assert_eq!(
        entry_count_after, 1,
        "metadata cache should still have one entry after a cache hit"
    );
}
