use std::sync::Arc;

use bytes::Bytes;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt};

use micromegas_range_cache::backend::RangeCacheBackend;
use micromegas_range_cache::memory_backend::MemoryBackend;
use micromegas_range_cache::range_cache::{DEFAULT_BLOCK_SIZE, RangeCache};

fn make_cache(origin: Arc<dyn ObjectStore>) -> RangeCache {
    let backend = Arc::new(MemoryBackend::new());
    RangeCache::new(origin, backend, DEFAULT_BLOCK_SIZE, "test".to_string())
}

async fn put_bytes(store: &InMemory, key: &str, data: &[u8]) {
    store
        .put(&Path::from(key), Bytes::copy_from_slice(data).into())
        .await
        .expect("put");
}

#[tokio::test]
async fn get_range_matches_direct() {
    let store = Arc::new(InMemory::new());
    let data: Vec<u8> = (0u8..=255).cycle().take(3 * 1024 * 1024).collect();
    put_bytes(&store, "test/obj", &data).await;

    let cache = make_cache(store.clone() as Arc<dyn ObjectStore>);
    let got = cache
        .get_range("test/obj", 500_000..2_500_000)
        .await
        .expect("get_range");
    assert_eq!(&got[..], &data[500_000..2_500_000]);
}

#[tokio::test]
async fn cold_read_populates_backend() {
    let store = Arc::new(InMemory::new());
    let data: Vec<u8> = vec![42u8; 2 * 1024 * 1024];
    put_bytes(&store, "obj", &data).await;

    let backend = Arc::new(MemoryBackend::new());
    let cache = RangeCache::new(
        store.clone() as Arc<dyn ObjectStore>,
        backend.clone(),
        DEFAULT_BLOCK_SIZE,
        "test".to_string(),
    );

    let got1 = cache.get_range("obj", 0..1024).await.expect("get_range 1");
    let got2 = cache.get_range("obj", 0..1024).await.expect("get_range 2");

    assert_eq!(got1, got2);
    assert_eq!(&got1[..], &data[..1024]);

    let blk_key = "blk:test:obj:0".to_string();
    assert!(
        backend.get(&blk_key).await.is_some(),
        "block should be in backend"
    );
}

#[tokio::test]
async fn warm_read_does_not_refetch_cached_blocks() {
    let store = Arc::new(InMemory::new());
    let data: Vec<u8> = vec![7u8; 1024 * 1024];
    put_bytes(&store, "file", &data).await;

    let backend = Arc::new(MemoryBackend::new());
    let cache = RangeCache::new(
        store.clone() as Arc<dyn ObjectStore>,
        backend.clone(),
        DEFAULT_BLOCK_SIZE,
        "ns".to_string(),
    );

    cache
        .get_range("file", 0..512 * 1024)
        .await
        .expect("first read");
    store
        .delete(&Path::from("file"))
        .await
        .expect("delete origin");
    let got = cache
        .get_range("file", 0..512 * 1024)
        .await
        .expect("second read from cache");
    assert_eq!(got.len(), 512 * 1024);
}

#[tokio::test]
async fn get_ranges_returns_correct_bytes() {
    let store = Arc::new(InMemory::new());
    let data: Vec<u8> = (0u8..255).cycle().take(4 * 1024 * 1024).collect();
    put_bytes(&store, "multi", &data).await;

    let cache = make_cache(store.clone() as Arc<dyn ObjectStore>);

    let ranges = vec![
        0u64..512_000u64,
        1_500_000u64..2_000_000u64,
        3_900_000u64..4_194_304u64,
    ];
    let results = cache
        .get_ranges("multi", &ranges)
        .await
        .expect("get_ranges");

    assert_eq!(results.len(), 3);
    assert_eq!(&results[0][..], &data[0..512_000]);
    assert_eq!(&results[1][..], &data[1_500_000..2_000_000]);
    assert_eq!(&results[2][..], &data[3_900_000..4_194_304]);
}

#[tokio::test]
async fn size_returns_file_size() {
    let store = Arc::new(InMemory::new());
    put_bytes(&store, "sized_file", &[0u8; 12345]).await;

    let cache = make_cache(store.clone() as Arc<dyn ObjectStore>);
    let size = cache.size("sized_file").await.expect("size");
    assert_eq!(size, 12345);
}
