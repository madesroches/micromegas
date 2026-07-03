use micromegas_object_cache::prefetch::{PrefetchItem, PrefetchResponse};

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
