//! Tests for `MICROMEGAS_PROCESS_PROPERTIES` parsing and merge precedence.
//!
//! The merge helper is tested rather than `TelemetryGuardBuilder::build()`: the
//! guard is behind a process-wide `lazy_static` weak handle, so exercising it
//! would require mutating process env and would only ever build once per test
//! binary.
use std::collections::HashMap;

use micromegas_telemetry_sink::{PROCESS_PROPERTIES_ENV_VAR, merge_process_properties};

fn merge(raw: &str) -> HashMap<String, String> {
    let mut properties = HashMap::new();
    merge_process_properties(&mut properties, raw).expect("merging valid properties");
    properties
}

#[test]
fn env_var_name_is_the_documented_one() {
    assert_eq!(PROCESS_PROPERTIES_ENV_VAR, "MICROMEGAS_PROCESS_PROPERTIES");
}

#[test]
fn parses_comma_separated_pairs() {
    let properties = merge("cluster=prod,role=cache");
    assert_eq!(properties.len(), 2);
    assert_eq!(properties.get("cluster").map(String::as_str), Some("prod"));
    assert_eq!(properties.get("role").map(String::as_str), Some("cache"));
}

#[test]
fn empty_input_adds_nothing() {
    assert!(merge("").is_empty());
}

/// A k8s manifest rendering an unset optional var produces `""`, and hand-written
/// lists routinely carry a trailing comma; neither should be an error.
#[test]
fn blank_entries_are_dropped() {
    let properties = merge(",cluster=prod,,");
    assert_eq!(properties.len(), 1);
    assert_eq!(properties.get("cluster").map(String::as_str), Some("prod"));
}

#[test]
fn value_may_be_empty() {
    let properties = merge("cluster=");
    assert_eq!(properties.get("cluster").map(String::as_str), Some(""));
}

/// A space after a comma is a common hand-editing slip; without trimming it would
/// silently produce a `" role"` key that no query would ever match.
#[test]
fn surrounding_whitespace_is_trimmed() {
    let properties = merge("cluster=prod, role=cache , zone = eu ");
    assert_eq!(properties.get("cluster").map(String::as_str), Some("prod"));
    assert_eq!(properties.get("role").map(String::as_str), Some("cache"));
    assert_eq!(properties.get("zone").map(String::as_str), Some("eu"));
}

/// Only the first `=` separates; the rest belongs to the value.
#[test]
fn value_may_contain_equals_signs() {
    let properties = merge("query=a=b=c");
    assert_eq!(properties.get("query").map(String::as_str), Some("a=b=c"));
}

/// Precedence: explicit `with_process_property()` calls win over the env var, so
/// an operator cannot spoof the `version` the `micromegas_main` macro stamps.
#[test]
fn existing_properties_are_not_overwritten() {
    let mut properties = HashMap::new();
    properties.insert("version".to_string(), "1.2.3".to_string());
    merge_process_properties(&mut properties, "version=999,cluster=prod")
        .expect("merging over an existing key");
    assert_eq!(properties.get("version").map(String::as_str), Some("1.2.3"));
    assert_eq!(properties.get("cluster").map(String::as_str), Some("prod"));
}

/// Within one env var, the first occurrence wins, consistent with the rule above.
#[test]
fn first_occurrence_of_a_duplicate_key_wins() {
    let properties = merge("cluster=first,cluster=second");
    assert_eq!(properties.get("cluster").map(String::as_str), Some("first"));
}

#[test]
fn entry_without_equals_is_rejected() {
    let mut properties = HashMap::new();
    let error = merge_process_properties(&mut properties, "cluster")
        .expect_err("an entry without '=' must be rejected");
    assert!(
        format!("{error:#}").contains("cluster"),
        "error should name the offending entry: {error:#}"
    );
}

#[test]
fn empty_key_is_rejected() {
    let mut properties = HashMap::new();
    merge_process_properties(&mut properties, "=prod").expect_err("an empty key must be rejected");
}

/// The ingestion service strips `micromegas.`-prefixed properties on write, so
/// rejecting them here turns a silent server-side drop into a startup failure.
#[test]
fn reserved_namespace_key_is_rejected() {
    let mut properties = HashMap::new();
    let error = merge_process_properties(&mut properties, "micromegas.audience=team")
        .expect_err("the reserved namespace must be rejected");
    assert!(
        format!("{error:#}").contains("micromegas."),
        "error should name the reserved prefix: {error:#}"
    );
}

/// Fail-fast means fail whole: a malformed entry must not leave half the pairs
/// applied, so the caller's map is either fully updated or untouched.
#[test]
fn a_rejected_entry_leaves_the_map_untouched() {
    let mut properties = HashMap::new();
    merge_process_properties(&mut properties, "cluster=prod,malformed")
        .expect_err("a malformed entry must be rejected");
    assert!(
        properties.is_empty(),
        "nothing should be merged when parsing fails: {properties:?}"
    );
}
