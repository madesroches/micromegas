use async_trait::async_trait;
use micromegas_object_cache::prefetch::{
    ObjectPrefetch, PrefetchItem, PrefetchResponse, PrefixPrefetch,
};
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::prefix::PrefixStore;
use object_store::{ObjectStore, ObjectStoreExt};
use std::sync::{Arc, Mutex};

#[test]
fn prefetch_item_whole_object_round_trip() {
    let item = PrefetchItem {
        key: "blobs/a".to_string(),
        size: 4096,
        ranges: None,
    };
    let json = serde_json::to_string(&item).expect("serialize");
    assert!(!json.contains("ranges"), "None ranges must be omitted");
    let back: PrefetchItem = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.key, item.key);
    assert_eq!(back.size, item.size);
    assert_eq!(back.ranges, None);
}

#[test]
fn prefetch_item_ranged_round_trip() {
    let item = PrefetchItem {
        key: "blobs/b".to_string(),
        size: 8192,
        ranges: Some(vec![[0, 1024], [4096, 8192]]),
    };
    let json = serde_json::to_string(&item).expect("serialize");
    let back: PrefetchItem = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.ranges, item.ranges);
}

#[test]
fn prefetch_item_missing_ranges_field_defaults_to_none() {
    let json = r#"{"key":"blobs/c","size":10}"#;
    let item: PrefetchItem = serde_json::from_str(json).expect("deserialize");
    assert_eq!(item.ranges, None);
}

#[test]
fn prefetch_response_round_trip() {
    let resp = PrefetchResponse {
        accepted: 3,
        rejected: 1,
        dropped: 2,
    };
    let json = serde_json::to_string(&resp).expect("serialize");
    let back: PrefetchResponse = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.accepted, 3);
    assert_eq!(back.rejected, 1);
    assert_eq!(back.dropped, 2);
}

/// Mock `ObjectPrefetch` that records the items it received.
#[derive(Debug, Default)]
struct RecordingPrefetch {
    received: Mutex<Vec<PrefetchItem>>,
}

#[async_trait]
impl ObjectPrefetch for RecordingPrefetch {
    async fn prefetch(&self, items: Vec<PrefetchItem>) -> anyhow::Result<PrefetchResponse> {
        let accepted = items.len();
        self.received.lock().expect("lock").extend(items);
        Ok(PrefetchResponse {
            accepted,
            rejected: 0,
            dropped: 0,
        })
    }
}

fn make_item(key: &str) -> PrefetchItem {
    PrefetchItem {
        key: key.to_string(),
        size: 42,
        ranges: None,
    }
}

#[tokio::test]
async fn prefix_prefetch_prepends_non_empty_root() {
    let recorder = Arc::new(RecordingPrefetch::default());
    let prefix_prefetch = PrefixPrefetch::new(recorder.clone(), Path::from("root"));

    let resp = prefix_prefetch
        .prefetch(vec![make_item("views/foo.parquet")])
        .await
        .expect("prefetch");
    assert_eq!(resp.accepted, 1);

    let received = recorder.received.lock().expect("lock");
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].key, "root/views/foo.parquet");
}

#[tokio::test]
async fn prefix_prefetch_leaves_key_unchanged_for_empty_root() {
    let recorder = Arc::new(RecordingPrefetch::default());
    let prefix_prefetch = PrefixPrefetch::new(recorder.clone(), Path::default());

    prefix_prefetch
        .prefetch(vec![make_item("views/foo.parquet")])
        .await
        .expect("prefetch");

    let received = recorder.received.lock().expect("lock");
    assert_eq!(received[0].key, "views/foo.parquet");
}

/// Locks in the "matches read key" contract: the key `PrefixPrefetch` produces
/// for a warm must equal the key a demand read produces through
/// `object_store::PrefixStore` for the same lake-root-relative path. We prove
/// this by putting an object directly on the raw store at the key
/// `PrefixPrefetch` computed, then reading it back *through* `PrefixStore`
/// using the original lake-root-relative path — if the keys didn't match,
/// this read would 404.
#[tokio::test]
async fn prefix_prefetch_key_matches_prefix_store_read_key() {
    let root = Path::from("root");
    let relative_key = "views/foo.parquet";

    let recorder = Arc::new(RecordingPrefetch::default());
    let prefix_prefetch = PrefixPrefetch::new(recorder.clone(), root.clone());
    prefix_prefetch
        .prefetch(vec![make_item(relative_key)])
        .await
        .expect("prefetch");
    let warmed_key = recorder.received.lock().expect("lock")[0].key.clone();

    let raw: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    raw.put(
        &Path::from(warmed_key.as_str()),
        bytes::Bytes::from("data").into(),
    )
    .await
    .expect("put at warmed key");

    let prefix_store = PrefixStore::new(raw, root);
    let get_result = prefix_store.get(&Path::from(relative_key)).await.expect(
        "a demand read through PrefixStore for the same relative key must hit the warmed key",
    );
    let bytes = get_result.bytes().await.expect("read bytes");
    assert_eq!(&bytes[..], b"data");
}
