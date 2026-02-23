use micromegas_tracing::process_info::ProcessInfo;
use std::collections::HashMap;

#[test]
fn round_trip_with_none_parent() {
    let info = ProcessInfo {
        process_id: uuid::Uuid::new_v4(),
        exe: "test".into(),
        username: "user".into(),
        realname: "real".into(),
        computer: "host".into(),
        distro: "linux".into(),
        cpu_brand: "cpu".into(),
        tsc_frequency: 1000,
        start_time: chrono::Utc::now(),
        start_ticks: 0,
        parent_process_id: None,
        properties: HashMap::new(),
    };
    let json = serde_json::to_string(&info).expect("serialize");
    let deserialized: ProcessInfo = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deserialized.parent_process_id, None);
}

#[test]
fn round_trip_with_some_parent() {
    let parent_id = uuid::Uuid::new_v4();
    let info = ProcessInfo {
        process_id: uuid::Uuid::new_v4(),
        exe: "test".into(),
        username: "user".into(),
        realname: "real".into(),
        computer: "host".into(),
        distro: "linux".into(),
        cpu_brand: "cpu".into(),
        tsc_frequency: 1000,
        start_time: chrono::Utc::now(),
        start_ticks: 0,
        parent_process_id: Some(parent_id),
        properties: HashMap::new(),
    };
    let json = serde_json::to_string(&info).expect("serialize");
    let deserialized: ProcessInfo = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deserialized.parent_process_id, Some(parent_id));
}

#[test]
fn deserialize_empty_string_parent_as_none() {
    // Simulates a 0.20.0 client that serializes None as ""
    let json = r#"{
        "process_id": "550e8400-e29b-41d4-a716-446655440000",
        "exe": "test",
        "username": "user",
        "realname": "real",
        "computer": "host",
        "distro": "linux",
        "cpu_brand": "cpu",
        "tsc_frequency": 1000,
        "start_time": "2025-01-01T00:00:00Z",
        "start_ticks": 0,
        "parent_process_id": "",
        "properties": {}
    }"#;
    let deserialized: ProcessInfo = serde_json::from_str(json).expect("deserialize");
    assert_eq!(deserialized.parent_process_id, None);
}

#[test]
fn deserialize_null_parent_as_none() {
    let json = r#"{
        "process_id": "550e8400-e29b-41d4-a716-446655440000",
        "exe": "test",
        "username": "user",
        "realname": "real",
        "computer": "host",
        "distro": "linux",
        "cpu_brand": "cpu",
        "tsc_frequency": 1000,
        "start_time": "2025-01-01T00:00:00Z",
        "start_ticks": 0,
        "parent_process_id": null,
        "properties": {}
    }"#;
    let deserialized: ProcessInfo = serde_json::from_str(json).expect("deserialize");
    assert_eq!(deserialized.parent_process_id, None);
}
