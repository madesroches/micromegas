use micromegas::tracing::property_set::property_get;
use redis_exporter::info_parser::ParsedInfo;
use redis_exporter::sampler::{build_base_properties, emit_command_stats, emit_info_metrics};

const FIXTURE: &str = include_str!("fixtures/info_all.txt");

#[test]
fn base_properties_carry_instance_and_extras() {
    let props = build_base_properties(
        "myhost:6379",
        &[("cluster".to_string(), "eu-west".to_string())],
    );
    let list = props.get_properties();
    assert_eq!(property_get(list, "instance"), Some("myhost:6379"));
    assert_eq!(property_get(list, "cluster"), Some("eu-west"));
    assert_eq!(list.len(), 2);
}

// No telemetry guard is installed in tests: emission dispatches into the
// void. These are smoke tests — the mapping code must not panic on real
// fixture data or on an empty INFO response.
#[test]
fn emit_on_fixture_does_not_panic() {
    let info = ParsedInfo::parse(FIXTURE);
    let props = build_base_properties("myhost:6379", &[]);
    emit_info_metrics(&info, props, 1_755_640_100);
    emit_command_stats(&info, props);
}

#[test]
fn emit_on_empty_info_does_not_panic() {
    let info = ParsedInfo::parse("");
    let props = build_base_properties("myhost:6379", &[]);
    emit_info_metrics(&info, props, 0);
    emit_command_stats(&info, props);
}

/// Full round against a live Redis; set MICROMEGAS_REDIS_EXPORTER_TEST_URL
/// (e.g. redis://127.0.0.1:6379) to enable. Skipped in CI (no Redis there).
#[tokio::test]
async fn integration_full_sample_against_live_redis() {
    let Ok(url) = std::env::var("MICROMEGAS_REDIS_EXPORTER_TEST_URL") else {
        eprintln!("MICROMEGAS_REDIS_EXPORTER_TEST_URL not set; skipping");
        return;
    };
    let client = redis::Client::open(url.as_str()).expect("opening redis client");
    let mut conn = redis::aio::ConnectionManager::new(client)
        .await
        .expect("connecting to live redis");
    let props = build_base_properties("integration-test", &[]);
    redis_exporter::sampler::sample_once(
        &mut conn,
        redis_exporter::cli::MetricsPreset::Full,
        props,
        0,
    )
    .await
    .expect("full sample against live redis");
}
