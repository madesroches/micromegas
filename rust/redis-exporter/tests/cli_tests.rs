use clap::Parser;
use redis_exporter::cli::{Cli, MetricsPreset, parse_properties};

#[test]
fn defaults() {
    let cli = Cli::try_parse_from(["micromegas-redis-exporter"]).expect("parsing empty args");
    let config = cli
        .into_config(None)
        .expect("building config from defaults");
    assert_eq!(config.target_name, "127.0.0.1:6379");
    assert_eq!(config.preset, MetricsPreset::Full);
    assert_eq!(config.sample_interval.as_secs(), 1);
    assert!(config.properties.is_empty());
    assert!(config.health_listen_addr.is_none());
}

#[test]
fn preset_ordering() {
    assert!(MetricsPreset::Core < MetricsPreset::Extended);
    assert!(MetricsPreset::Extended < MetricsPreset::Full);
}

#[test]
fn target_name_derived_from_url_without_credentials() {
    let cli = Cli::try_parse_from([
        "micromegas-redis-exporter",
        "--redis-url",
        "redis://user:secret@myhost:6390/2",
    ])
    .expect("parsing url arg");
    let config = cli.into_config(None).expect("building config");
    assert_eq!(config.target_name, "myhost:6390");
}

#[test]
fn explicit_target_name_wins() {
    let cli = Cli::try_parse_from([
        "micromegas-redis-exporter",
        "--target-name",
        "cache-eu-west",
    ])
    .expect("parsing target-name arg");
    let config = cli.into_config(None).expect("building config");
    assert_eq!(config.target_name, "cache-eu-west");
}

#[test]
fn properties_parse_into_pairs() {
    let pairs = parse_properties(&["cluster=eu-west".into(), "role=cache".into()])
        .expect("parsing valid properties");
    assert_eq!(
        pairs,
        vec![
            ("cluster".to_string(), "eu-west".to_string()),
            ("role".to_string(), "cache".to_string())
        ]
    );
}

#[test]
fn property_without_equals_is_rejected() {
    let err = parse_properties(&["oops".into()]).expect_err("bare word must be rejected");
    assert!(err.to_string().contains("key=value"), "got: {err}");
}

#[test]
fn instance_property_is_reserved() {
    let err = parse_properties(&["instance=x".into()]).expect_err("instance must be reserved");
    assert!(err.to_string().contains("instance"), "got: {err}");
}

#[test]
fn reserved_properties_are_rejected_case_insensitively() {
    for entry in ["Instance=x", "COMMAND=x", "Db=0", "eVeNt=x"] {
        parse_properties(&[entry.into()]).expect_err(&format!("{entry} must be reserved"));
    }
}

#[test]
fn command_db_event_properties_are_reserved() {
    for key in ["command", "db", "event"] {
        let entry = format!("{key}=x");
        let err = parse_properties(&[entry]).expect_err(&format!("{key} must be reserved"));
        assert!(err.to_string().contains(key), "got: {err}");
    }
}

#[test]
fn empty_property_entries_are_ignored() {
    let pairs = parse_properties(&["".into()]).expect("empty entry must be dropped, not an error");
    assert!(pairs.is_empty());
}

#[test]
fn empty_properties_env_value_does_not_fail_cli_parse() {
    // clap's value_delimiter turns an empty env value into one empty item;
    // this must not fail startup (k8s templates render unset optional vars
    // as empty strings).
    let cli = Cli::try_parse_from(["micromegas-redis-exporter", "--property", ""])
        .expect("parsing empty --property value");
    let config = cli.into_config(None).expect("building config");
    assert!(config.properties.is_empty());
}

#[test]
fn password_override_applies() {
    let cli = Cli::try_parse_from(["micromegas-redis-exporter"]).expect("parsing empty args");
    let config = cli
        .into_config(Some("s3cret".to_string()))
        .expect("building config with password");
    assert_eq!(
        config.connection_info.redis_settings().password(),
        Some("s3cret")
    );
}

#[test]
fn zero_interval_is_rejected() {
    let cli = Cli::try_parse_from([
        "micromegas-redis-exporter",
        "--sample-interval-seconds",
        "0",
    ])
    .expect("parsing interval arg");
    cli.into_config(None)
        .expect_err("zero interval must be rejected");
}
