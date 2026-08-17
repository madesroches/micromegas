//! Unit tests (no DB) for `WriteAudience` and the two `pub` property-stamping helpers
//! (AbAC Stage 5, #1373). Every case here is a pure function of its arguments.

use micromegas_ingestion::web_ingestion_service::{
    finalize_process_properties, strip_reserved_properties,
};
use micromegas_ingestion::write_audience::WriteAudience;
use micromegas_telemetry::property::{PROPERTY_AUDIENCE, Property};
use std::sync::Arc;

fn prop(key: &str, value: &str) -> Property {
    Property::new(Arc::new(key.to_string()), Arc::new(value.to_string()))
}

fn find<'a>(properties: &'a [Property], key: &str) -> Option<&'a Property> {
    properties.iter().find(|p| p.key_str() == key)
}

// ---------------------------------------------------------------------------
// WriteAudience::new
// ---------------------------------------------------------------------------

#[test]
fn write_audience_none_carries_no_audience() {
    let audience = WriteAudience::none();
    assert_eq!(audience.as_str(), None);
}

#[test]
fn write_audience_new_none_is_equivalent_to_none() {
    let audience = WriteAudience::new(None).expect("None is always valid");
    assert_eq!(audience, WriteAudience::none());
}

#[test]
fn write_audience_accepts_the_full_charset() {
    for valid in ["team-alpha", "team_alpha", "TeamAlpha123", "a", "-", "_"] {
        let audience = WriteAudience::new(Some(valid))
            .unwrap_or_else(|e| panic!("{valid:?} should be a valid audience: {e:#}"));
        assert_eq!(audience.as_str(), Some(valid));
    }
}

#[test]
fn write_audience_rejects_empty_string() {
    assert!(WriteAudience::new(Some("")).is_err());
}

#[test]
fn write_audience_rejects_disallowed_characters() {
    for invalid in [
        "team:alpha",
        "team alpha",
        "team.alpha",
        "team/alpha",
        "team@alpha",
    ] {
        assert!(
            WriteAudience::new(Some(invalid)).is_err(),
            "{invalid:?} must be rejected"
        );
    }
}

#[test]
fn write_audience_rejects_256_bytes() {
    let too_long = "a".repeat(256);
    assert!(WriteAudience::new(Some(&too_long)).is_err());
    let exactly_255 = "a".repeat(255);
    assert!(WriteAudience::new(Some(&exactly_255)).is_ok());
}

#[test]
fn write_audience_rejects_non_ascii() {
    assert!(WriteAudience::new(Some("team-\u{e9}")).is_err()); // "team-é"
}

#[test]
fn write_audience_default_is_none() {
    assert_eq!(WriteAudience::default(), WriteAudience::none());
}

// ---------------------------------------------------------------------------
// strip_reserved_properties
// ---------------------------------------------------------------------------

#[test]
fn strip_reserved_properties_drops_only_the_reserved_namespace() {
    let input = vec![
        prop("micromegas.audience", "client-asserted"),
        prop("micromegas.something-else", "also dropped"),
        prop("otel.resource.service.name", "kept"),
        prop("arbitrary-client-key", "kept"),
    ];
    let out = strip_reserved_properties(input);
    assert_eq!(out.len(), 2);
    assert!(find(&out, "micromegas.audience").is_none());
    assert!(find(&out, "micromegas.something-else").is_none());
    assert_eq!(
        find(&out, "otel.resource.service.name").map(|p| p.value_str()),
        Some("kept")
    );
    assert_eq!(
        find(&out, "arbitrary-client-key").map(|p| p.value_str()),
        Some("kept")
    );
}

#[test]
fn strip_reserved_properties_on_empty_input_is_empty() {
    assert!(strip_reserved_properties(vec![]).is_empty());
}

#[test]
fn strip_reserved_properties_keeps_a_key_that_merely_contains_the_prefix_mid_string() {
    // Only a *prefix* match is reserved -- a key that happens to contain "micromegas." later in
    // the string, but doesn't start with it, is an ordinary client key.
    let input = vec![prop("client.micromegas.audience", "kept")];
    let out = strip_reserved_properties(input);
    assert_eq!(out.len(), 1);
}

// ---------------------------------------------------------------------------
// finalize_process_properties
// ---------------------------------------------------------------------------

#[test]
fn finalize_process_properties_drops_client_stamp_and_writes_the_authenticated_one() {
    let client = vec![prop("micromegas.audience", "attacker-asserted")];
    let audience = WriteAudience::new(Some("team-a")).unwrap();
    let out = finalize_process_properties(client, &audience);
    assert_eq!(out.len(), 1);
    assert_eq!(
        find(&out, PROPERTY_AUDIENCE).map(|p| p.value_str()),
        Some("team-a"),
        "the authenticated audience must win, not the client-asserted one"
    );
}

#[test]
fn finalize_process_properties_drops_other_reserved_keys_and_keeps_the_rest() {
    let client = vec![
        prop("micromegas.audience", "attacker-asserted"),
        prop("micromegas.other-reserved", "attacker-asserted"),
        prop("otel.resource.service.name", "kept"),
        prop("arbitrary-client-key", "kept"),
    ];
    let audience = WriteAudience::new(Some("team-a")).unwrap();
    let out = finalize_process_properties(client, &audience);
    // otel.resource.service.name + arbitrary-client-key + the one stamped micromegas.audience
    assert_eq!(out.len(), 3);
    assert!(find(&out, "micromegas.other-reserved").is_none());
    assert_eq!(
        find(&out, "otel.resource.service.name").map(|p| p.value_str()),
        Some("kept")
    );
    assert_eq!(
        find(&out, "arbitrary-client-key").map(|p| p.value_str()),
        Some("kept")
    );
    assert_eq!(
        find(&out, PROPERTY_AUDIENCE).map(|p| p.value_str()),
        Some("team-a")
    );
}

#[test]
fn finalize_process_properties_with_none_audience_writes_no_property_at_all() {
    let client = vec![prop("arbitrary-client-key", "kept")];
    let out = finalize_process_properties(client, &WriteAudience::none());
    assert_eq!(out.len(), 1, "only the untouched client key remains");
    assert!(
        find(&out, PROPERTY_AUDIENCE).is_none(),
        "an unstamped write must leave the audience property absent, not present-and-empty"
    );
}

#[test]
fn finalize_process_properties_with_none_audience_still_strips_client_micromegas_star() {
    let client = vec![
        prop("micromegas.audience", "self-stamped-pre-stage-5"),
        prop("arbitrary-client-key", "kept"),
    ];
    let out = finalize_process_properties(client, &WriteAudience::none());
    assert_eq!(out.len(), 1);
    assert!(
        find(&out, PROPERTY_AUDIENCE).is_none(),
        "a pre-Stage-5 self-stamp must not survive once ingesting under an audience-less \
         credential -- it becomes unstamped, not a retained self-assertion"
    );
}

#[test]
fn finalize_process_properties_leaves_otel_resource_and_arbitrary_keys_untouched() {
    let client = vec![
        prop("otel.resource.host.name", "my-host"),
        prop("otel.resource.process.pid", "1234"),
        prop("some-native-client-key", "value"),
    ];
    let audience = WriteAudience::new(Some("team-a")).unwrap();
    let out = finalize_process_properties(client, &audience);
    assert_eq!(out.len(), 4); // 3 untouched + the stamp
    assert_eq!(
        find(&out, "otel.resource.host.name").map(|p| p.value_str()),
        Some("my-host")
    );
    assert_eq!(
        find(&out, "otel.resource.process.pid").map(|p| p.value_str()),
        Some("1234")
    );
    assert_eq!(
        find(&out, "some-native-client-key").map(|p| p.value_str()),
        Some("value")
    );
}
