//! Unit tests (no DB) for `WriteAudience` and the two `pub` property-stamping helpers
//! (AbAC Stage 5, #1373; collapsed to a single state by #1519). Every case here is a pure
//! function of its arguments.

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
fn write_audience_accepts_the_full_charset() {
    for valid in ["team-alpha", "team_alpha", "TeamAlpha123", "a", "-", "_"] {
        let audience = WriteAudience::new(valid)
            .unwrap_or_else(|e| panic!("{valid:?} should be a valid audience: {e:#}"));
        assert_eq!(audience.as_str(), valid);
    }
}

#[test]
fn write_audience_rejects_empty_string() {
    assert!(WriteAudience::new("").is_err());
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
            WriteAudience::new(invalid).is_err(),
            "{invalid:?} must be rejected"
        );
    }
}

#[test]
fn write_audience_rejects_256_bytes() {
    let too_long = "a".repeat(256);
    assert!(WriteAudience::new(&too_long).is_err());
    let exactly_255 = "a".repeat(255);
    assert!(WriteAudience::new(&exactly_255).is_ok());
}

#[test]
fn write_audience_rejects_non_ascii() {
    assert!(WriteAudience::new("team-\u{e9}").is_err()); // "team-é"
}

// ---------------------------------------------------------------------------
// WriteAudience::id_namespace
// ---------------------------------------------------------------------------

#[test]
fn id_namespace_is_none_when_equal_to_the_default() {
    let default = WriteAudience::new("public").expect("valid audience");
    let same = WriteAudience::new("public").expect("valid audience");
    assert_eq!(same.id_namespace(&default), None);
}

#[test]
fn id_namespace_is_some_of_itself_when_different_from_the_default() {
    let default = WriteAudience::new("public").expect("valid audience");
    let other = WriteAudience::new("team-a").expect("valid audience");
    assert_eq!(other.id_namespace(&default), Some("team-a"));
}

#[test]
fn id_namespace_rule_holds_independent_of_which_label_is_the_default() {
    // The rule is "equal to the default", not "equal to `public`" -- verify both cases again
    // with a non-`public` label as the deployment default.
    let default = WriteAudience::new("unassigned").expect("valid audience");
    let same = WriteAudience::new("unassigned").expect("valid audience");
    assert_eq!(same.id_namespace(&default), None);

    let other = WriteAudience::new("public").expect("valid audience");
    assert_eq!(other.id_namespace(&default), Some("public"));
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
    let audience = WriteAudience::new("team-a").unwrap();
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
    let audience = WriteAudience::new("team-a").unwrap();
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
fn finalize_process_properties_stamps_the_resolved_default_like_any_other_audience() {
    // A caller resolved to the deployment default (an audience-less credential, resolved by
    // `resolve_write_audience` before this function ever sees it) stamps `micromegas.audience`
    // exactly like an explicitly-bound one -- there is no separate unstamped path any more
    // (#1519).
    let client = vec![
        prop("micromegas.audience", "self-stamped-pre-stage-5"),
        prop("arbitrary-client-key", "kept"),
    ];
    let default = WriteAudience::new("public").unwrap();
    let out = finalize_process_properties(client, &default);
    assert_eq!(out.len(), 2);
    assert_eq!(
        find(&out, PROPERTY_AUDIENCE).map(|p| p.value_str()),
        Some("public"),
        "a resolved-to-default caller must be stamped, not left unstamped"
    );
    assert_eq!(
        find(&out, "arbitrary-client-key").map(|p| p.value_str()),
        Some("kept")
    );
}

#[test]
fn finalize_process_properties_leaves_otel_resource_and_arbitrary_keys_untouched() {
    let client = vec![
        prop("otel.resource.host.name", "my-host"),
        prop("otel.resource.process.pid", "1234"),
        prop("some-native-client-key", "value"),
    ];
    let audience = WriteAudience::new("team-a").unwrap();
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
