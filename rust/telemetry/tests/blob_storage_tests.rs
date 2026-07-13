use micromegas_telemetry::blob_storage::parse_object_store_url;

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
