// Tests for DataLakeConnection::warm_object — the fire-and-forget cache warming
// primitive (prefetch priority, #1201); the write-partition path is its first caller.
use async_trait::async_trait;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_object_cache::prefetch::{ObjectPrefetch, PrefetchItem, PrefetchResponse};
use micromegas_telemetry::blob_storage::BlobStorage;
use object_store::memory::InMemory;
use object_store::path::Path;
use std::sync::{Arc, Mutex};

/// Mock `ObjectPrefetch` that records received items and can be configured to
/// fail, so tests can assert both the happy path and that a failure doesn't
/// propagate out of the fire-and-forget task.
#[derive(Debug, Default)]
struct MockPrefetch {
    received: Mutex<Vec<PrefetchItem>>,
    fail: bool,
}

#[async_trait]
impl ObjectPrefetch for MockPrefetch {
    async fn prefetch(&self, items: Vec<PrefetchItem>) -> anyhow::Result<PrefetchResponse> {
        if self.fail {
            return Err(anyhow::anyhow!("mock prefetch failure"));
        }
        let accepted = items.len();
        self.received.lock().expect("lock").extend(items);
        Ok(PrefetchResponse {
            accepted,
            rejected: 0,
            dropped: 0,
        })
    }
}

fn make_pool() -> sqlx::PgPool {
    sqlx::PgPool::connect_lazy("postgres://localhost/unused")
        .expect("lazy pool creation is infallible")
}

fn make_blob_storage() -> Arc<BlobStorage> {
    Arc::new(BlobStorage::new(Arc::new(InMemory::new()), Path::default()))
}

#[tokio::test]
async fn warm_object_sends_one_item_when_cache_configured() {
    let mock = Arc::new(MockPrefetch::default());
    let lake = DataLakeConnection::new_with_prefetch(
        make_pool(),
        make_blob_storage(),
        Some(mock.clone() as Arc<dyn ObjectPrefetch>),
    );

    let handle = lake
        .warm_object("views/foo/bar/2024-01-01/00-00-00_id.parquet", 4096)
        .expect("warm_object should schedule a task when the cache is configured");
    handle.await.expect("warm task should not panic");

    let received = mock.received.lock().expect("lock");
    assert_eq!(received.len(), 1);
    assert_eq!(
        received[0].key,
        "views/foo/bar/2024-01-01/00-00-00_id.parquet"
    );
    assert_eq!(received[0].size, 4096);
    assert_eq!(received[0].ranges, None);
}

#[tokio::test]
async fn warm_object_noop_when_size_is_zero() {
    let mock = Arc::new(MockPrefetch::default());
    let lake = DataLakeConnection::new_with_prefetch(
        make_pool(),
        make_blob_storage(),
        Some(mock.clone() as Arc<dyn ObjectPrefetch>),
    );

    let handle = lake.warm_object("views/foo/bar/empty.parquet", 0);
    assert!(handle.is_none(), "nothing to warm when size is zero");
    assert!(mock.received.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn warm_object_noop_when_cache_not_configured() {
    let lake = DataLakeConnection::new(make_pool(), make_blob_storage());

    let handle = lake.warm_object("views/foo/bar/2024-01-01/00-00-00_id.parquet", 4096);
    assert!(
        handle.is_none(),
        "warm_object must be a no-op when the cache is not configured"
    );
}

#[tokio::test]
async fn warm_object_failure_does_not_propagate() {
    let mock = Arc::new(MockPrefetch {
        received: Mutex::new(Vec::new()),
        fail: true,
    });
    let lake = DataLakeConnection::new_with_prefetch(
        make_pool(),
        make_blob_storage(),
        Some(mock as Arc<dyn ObjectPrefetch>),
    );

    let handle = lake
        .warm_object("views/foo/bar/2024-01-01/00-00-00_id.parquet", 4096)
        .expect("warm_object should still schedule a task");
    // The task itself must complete cleanly (no panic / no propagated error);
    // a failed warm just means the first read stays a cold miss.
    handle
        .await
        .expect("warm task should not panic even when the prefetch call fails");
}
