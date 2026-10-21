use micromegas_auth::log_sanitize::sanitize_for_log;

#[test]
fn control_and_bidi_characters_are_replaced() {
    let out = sanitize_for_log("a\r\nb\tc\u{1b}[31md\u{202E}e\u{2066}f\u{2069}g", 100);
    assert_eq!(out, "a??b?c?[31md?e?f?g");
}

#[test]
fn truncation_never_splits_a_multibyte_character() {
    let out = sanitize_for_log("ééééé", 3);
    assert_eq!(out, "éé…");
    assert_eq!(out.chars().count(), 3);
}

#[test]
fn value_at_the_limit_is_not_truncated() {
    assert_eq!(sanitize_for_log("abc", 3), "abc");
}

#[test]
fn clean_short_value_is_unchanged() {
    assert_eq!(
        sanitize_for_log("Mozilla/5.0 (X11)", 256),
        "Mozilla/5.0 (X11)"
    );
}
