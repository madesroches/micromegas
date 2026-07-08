use bytes::Bytes;
use datafusion::arrow::array::Int32Array;
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::parquet::arrow::ArrowWriter;
use datafusion::parquet::file::metadata::ParquetMetaData;
use micromegas_analytics::lakehouse::caching_reader::CachingReader;
use micromegas_analytics::lakehouse::file_cache::FileCache;
use micromegas_analytics::lakehouse::metadata_cache::MetadataCache;
use micromegas_analytics::lakehouse::partition_metadata::load_partition_metadata;
use object_store::ObjectStoreExt;
use object_store::memory::InMemory;
use object_store::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// ============================================================================
// FileCache Tests
// ============================================================================

#[tokio::test]
async fn test_should_cache_threshold() {
    let cache = FileCache::new(100 * 1024, 10 * 1024); // 100KB cache, 10KB max file

    assert!(cache.should_cache(10 * 1024)); // exactly at threshold
    assert!(cache.should_cache(1024)); // below threshold
    assert!(!cache.should_cache(10 * 1024 + 1)); // above threshold
}

#[tokio::test]
async fn test_cache_hit_skips_loader() {
    let cache = FileCache::new(1024 * 1024, 100 * 1024);
    let load_count = Arc::new(AtomicUsize::new(0));

    let data = Bytes::from_static(b"test data");

    // First load
    let load_count_clone = Arc::clone(&load_count);
    let data_clone = data.clone();
    let result = cache
        .get_or_load("file1", 9, move || {
            load_count_clone.fetch_add(1, Ordering::SeqCst);
            let d = data_clone.clone();
            async move { Ok::<_, std::io::Error>(d) }
        })
        .await
        .expect("first load should succeed");
    assert_eq!(result, data);
    assert_eq!(load_count.load(Ordering::SeqCst), 1);

    // Second load - should hit cache
    let load_count_clone = Arc::clone(&load_count);
    let result = cache
        .get_or_load("file1", 9, move || {
            load_count_clone.fetch_add(1, Ordering::SeqCst);
            async move { Ok::<_, std::io::Error>(Bytes::new()) }
        })
        .await
        .expect("second load should succeed");
    assert_eq!(result, data);
    assert_eq!(load_count.load(Ordering::SeqCst), 1); // loader not called again
}

#[tokio::test]
async fn test_different_keys_both_load() {
    let cache = FileCache::new(1024 * 1024, 100 * 1024);
    let load_count = Arc::new(AtomicUsize::new(0));

    for key in ["file1", "file2"] {
        let load_count_clone = Arc::clone(&load_count);
        cache
            .get_or_load(key, 5, move || {
                load_count_clone.fetch_add(1, Ordering::SeqCst);
                async move { Ok::<_, std::io::Error>(Bytes::from_static(b"data")) }
            })
            .await
            .expect("load should succeed");
    }

    assert_eq!(load_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_loader_error_propagation() {
    let cache = FileCache::new(1024 * 1024, 100 * 1024);

    let result: Result<Bytes, _> = cache
        .get_or_load("file1", 5, || async {
            Err::<Bytes, _>(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "not found",
            ))
        })
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_stats_accuracy() {
    let cache = FileCache::new(1024 * 1024, 100 * 1024);

    assert_eq!(cache.stats(), (0, 0));

    cache
        .get_or_load("file1", 100, || async {
            Ok::<_, std::io::Error>(Bytes::from(vec![0u8; 100]))
        })
        .await
        .expect("load should succeed");

    // Run pending tasks to ensure stats are up-to-date
    cache.run_pending_tasks().await;

    let (count, size) = cache.stats();
    assert_eq!(count, 1);
    assert_eq!(size, 100);
}

#[tokio::test]
async fn test_thundering_herd_single_load() {
    let cache = Arc::new(FileCache::new(1024 * 1024, 100 * 1024));
    let load_count = Arc::new(AtomicUsize::new(0));

    // Spawn 10 concurrent requests for the same key
    let handles: Vec<_> = (0..10)
        .map(|_| {
            let cache = Arc::clone(&cache);
            let load_count = Arc::clone(&load_count);
            tokio::spawn(async move {
                cache
                    .get_or_load("same_key", 5, || {
                        let lc = Arc::clone(&load_count);
                        async move {
                            lc.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                            Ok::<_, std::io::Error>(Bytes::from_static(b"data"))
                        }
                    })
                    .await
            })
        })
        .collect();

    for handle in handles {
        handle
            .await
            .expect("join should succeed")
            .expect("load should succeed");
    }

    // With thundering herd protection, loader should be called exactly once
    assert_eq!(load_count.load(Ordering::SeqCst), 1);
}

// ============================================================================
// CachingReader Tests
// ============================================================================

async fn setup_test_store() -> (Arc<InMemory>, Path, Bytes) {
    let store = Arc::new(InMemory::new());
    let path = Path::from("test/file.parquet");
    let data = Bytes::from(vec![0u8; 1000]); // 1KB test file
    store
        .put(&path, data.clone().into())
        .await
        .expect("put should succeed");
    (store, path, data)
}

#[tokio::test]
async fn test_get_bytes_returns_correct_range() {
    let (store, path, data) = setup_test_store().await;
    let cache = Arc::new(FileCache::new(1024 * 1024, 100 * 1024));

    let mut reader = CachingReader::new(store, path.clone(), path.to_string(), 1000, cache);

    let result = reader
        .get_bytes(100..200)
        .await
        .expect("get_bytes should succeed");
    assert_eq!(result, data.slice(100..200));
}

#[tokio::test]
async fn test_get_byte_ranges_multiple() {
    let (store, path, data) = setup_test_store().await;
    let cache = Arc::new(FileCache::new(1024 * 1024, 100 * 1024));

    let mut reader = CachingReader::new(store, path.clone(), path.to_string(), 1000, cache);

    let ranges = vec![0..100, 500..600, 900..1000];
    let results = reader
        .get_byte_ranges(ranges.clone())
        .await
        .expect("get_byte_ranges should succeed");

    assert_eq!(results.len(), 3);
    for (result, range) in results.iter().zip(ranges.iter()) {
        assert_eq!(
            *result,
            data.slice(range.start as usize..range.end as usize)
        );
    }
}

#[tokio::test]
async fn test_large_file_bypasses_cache() {
    let store = Arc::new(InMemory::new());
    let path = Path::from("test/large.parquet");
    let large_data = Bytes::from(vec![0u8; 20 * 1024]); // 20KB (larger than 10KB threshold)
    store
        .put(&path, large_data.clone().into())
        .await
        .expect("put should succeed");

    let cache = Arc::new(FileCache::new(1024 * 1024, 10 * 1024)); // 10KB threshold

    let mut reader = CachingReader::new(
        store,
        path.clone(),
        path.to_string(),
        20 * 1024,
        cache.clone(),
    );

    // Read should succeed
    let result = reader
        .get_bytes(0..1000)
        .await
        .expect("get_bytes should succeed");
    assert_eq!(result.len(), 1000);

    // Cache should remain empty (file too large)
    assert_eq!(cache.stats().0, 0);
}

#[tokio::test]
async fn test_cached_read_populates_cache() {
    let (store, path, _data) = setup_test_store().await;
    let cache = Arc::new(FileCache::new(1024 * 1024, 100 * 1024));

    let mut reader = CachingReader::new(store, path.clone(), path.to_string(), 1000, cache.clone());

    // Initial cache should be empty
    assert_eq!(cache.stats().0, 0);

    // Read should populate cache
    reader
        .get_bytes(0..100)
        .await
        .expect("get_bytes should succeed");

    // Run pending tasks to ensure stats are up-to-date
    cache.run_pending_tasks().await;

    // Cache should now have the file
    assert_eq!(cache.stats().0, 1);
    assert_eq!(cache.stats().1, 1000); // full file size
}

#[tokio::test]
async fn test_multiple_readers_share_cache() {
    let (store, path, data) = setup_test_store().await;
    let cache = Arc::new(FileCache::new(1024 * 1024, 100 * 1024));

    // First reader reads a range
    let mut reader1 = CachingReader::new(
        store.clone(),
        path.clone(),
        path.to_string(),
        1000,
        cache.clone(),
    );
    let result1 = reader1
        .get_bytes(0..100)
        .await
        .expect("get_bytes should succeed");
    assert_eq!(result1, data.slice(0..100));

    // Second reader should benefit from the cache
    let mut reader2 =
        CachingReader::new(store, path.clone(), path.to_string(), 1000, cache.clone());
    let result2 = reader2
        .get_bytes(500..600)
        .await
        .expect("get_bytes should succeed");
    assert_eq!(result2, data.slice(500..600));

    // Run pending tasks to ensure stats are up-to-date
    cache.run_pending_tasks().await;

    // Cache should still have just 1 entry (same file)
    assert_eq!(cache.stats().0, 1);
}

// ============================================================================
// load_partition_metadata tests
// ============================================================================

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
    let store = Arc::new(InMemory::new());
    let path = Path::from("test/footer.parquet");
    let (data, from_direct_parse) = write_test_parquet();
    store
        .put(&path, data.clone().into())
        .await
        .expect("put should succeed");

    let cache = Arc::new(FileCache::new(1024 * 1024, 100 * 1024));
    let mut reader = CachingReader::new(
        store,
        path.clone(),
        path.to_string(),
        data.len() as u64,
        cache,
    );

    let from_footer = load_partition_metadata(&mut reader, path.as_ref(), data.len() as u64, None)
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
    let store = Arc::new(InMemory::new());
    let path = Path::from("test/footer_with_cache.parquet");
    let (data, _from_direct_parse) = write_test_parquet();
    store
        .put(&path, data.clone().into())
        .await
        .expect("put should succeed");

    let file_cache = Arc::new(FileCache::new(1024 * 1024, 100 * 1024));
    let metadata_cache = MetadataCache::new(1024 * 1024);

    // First call: metadata cache miss, should parse the footer and backfill the cache.
    let mut reader1 = CachingReader::new(
        store.clone(),
        path.clone(),
        path.to_string(),
        data.len() as u64,
        file_cache.clone(),
    );
    let first = load_partition_metadata(
        &mut reader1,
        path.as_ref(),
        data.len() as u64,
        Some(&metadata_cache),
    )
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
    // fresh footer read (a new reader is used only because CachingReader::get_bytes is &mut;
    // the cache hit path returns before touching the reader).
    let mut reader2 = CachingReader::new(
        store,
        path.clone(),
        path.to_string(),
        data.len() as u64,
        file_cache,
    );
    let second = load_partition_metadata(
        &mut reader2,
        path.as_ref(),
        data.len() as u64,
        Some(&metadata_cache),
    )
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
