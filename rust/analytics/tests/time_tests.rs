use chrono::DateTime;
use micromegas_analytics::arrow_properties::serialize_properties_to_jsonb;
use micromegas_analytics::metadata::ProcessMetadata;
use micromegas_analytics::time::make_time_converter_from_latest_timing;
use std::collections::HashMap;
use std::sync::Arc;

#[test]
fn test_make_time_converter_from_latest_timing() {
    let properties = HashMap::new();
    let properties_jsonb = serialize_properties_to_jsonb(&properties).unwrap();
    let process_info = ProcessMetadata {
        process_id: uuid::Uuid::new_v4(),
        exe: "test".to_string(),
        username: "test".to_string(),
        realname: "test".to_string(),
        computer: "test".to_string(),
        distro: "test".to_string(),
        cpu_brand: "test".to_string(),
        tsc_frequency: 0, // Force frequency estimation
        start_time: DateTime::from_timestamp_nanos(0),
        start_ticks: 0,
        parent_process_id: None,
        properties: Arc::new(properties_jsonb),
    };

    let last_block_end_ticks = 1_000_000; // 1M ticks
    let last_block_end_time = DateTime::from_timestamp_nanos(1_000_000_000); // 1 second

    let converter = make_time_converter_from_latest_timing(
        &process_info,
        last_block_end_ticks,
        last_block_end_time,
    )
    .expect("Should create converter");

    // Test that conversion is consistent
    let test_ticks = 500_000; // Half way
    let result_ns = converter.ticks_to_nanoseconds(test_ticks);
    let expected_ns = 500_000_000; // Half a second

    // Allow some rounding error
    assert!(
        (result_ns - expected_ns).abs() < 1000,
        "Expected ~{}, got {}",
        expected_ns,
        result_ns
    );
}

#[test]
fn test_make_time_converter_with_tsc_frequency() {
    let properties = HashMap::new();
    let properties_jsonb = serialize_properties_to_jsonb(&properties).unwrap();
    let process_info = ProcessMetadata {
        process_id: uuid::Uuid::new_v4(),
        exe: "test".to_string(),
        username: "test".to_string(),
        realname: "test".to_string(),
        computer: "test".to_string(),
        distro: "test".to_string(),
        cpu_brand: "test".to_string(),
        tsc_frequency: 1_000_000, // 1MHz TSC
        start_time: DateTime::from_timestamp_nanos(0),
        start_ticks: 0,
        parent_process_id: None,
        properties: Arc::new(properties_jsonb),
    };

    // When TSC frequency is available, last_block timing should be ignored
    let converter = make_time_converter_from_latest_timing(
        &process_info,
        999999, // Different timing data
        DateTime::from_timestamp_nanos(999999),
    )
    .expect("Should create converter");

    // Should use TSC frequency, not estimated frequency
    assert_eq!(converter.get_frequency(), 1_000_000);
}
