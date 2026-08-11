use bytes::Bytes;
use micromegas_telemetry::blob_storage::{BlobStorage, PutIfAbsent, parse_object_store_url};
use object_store::memory::InMemory;
use object_store::path::Path;
use std::sync::Arc;

#[test]
fn parse_object_store_url_file_scheme() {
    let dir = std::env::temp_dir().join("micromegas_parse_object_store_url_test");
    let dir_str = dir.to_str().expect("utf8 tmp dir path");
    let uri = format!("file://{dir_str}");

    let (_store, prefix) = parse_object_store_url(&uri).expect("parsing a file:// URI");

    // For `file://` URIs, object_store roots the store at `/` and returns the
    // path component (leading slash stripped) as the prefix.
    assert_eq!(prefix.as_ref(), dir_str.trim_start_matches('/'));
}

/// `put_if_absent` over `InMemory`: first write creates, second write (same key,
/// different bytes) is reported as a collision rather than applied, and the object
/// still holds the *first* write's bytes afterwards — the invariant this method
/// exists to enforce.
#[tokio::test]
async fn put_if_absent_create_then_collide_preserves_first_write() {
    let store = BlobStorage::new(Arc::new(InMemory::new()), Path::from(""));

    let first = store
        .put_if_absent("some/object", Bytes::from_static(b"first"))
        .await
        .expect("first write");
    assert_eq!(first, PutIfAbsent::Created);

    let second = store
        .put_if_absent("some/object", Bytes::from_static(b"second"))
        .await
        .expect("colliding write must be reported, not fail");
    assert_eq!(second, PutIfAbsent::AlreadyExists);

    let bytes = store
        .read_blob("some/object")
        .await
        .expect("reading back the object");
    assert_eq!(
        bytes.as_ref(),
        b"first",
        "a colliding put_if_absent must leave the original bytes untouched"
    );
}
