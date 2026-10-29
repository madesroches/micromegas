//! Pure unit tests for `key_selector::canonicalize_key_selector`.

use analytics_web_srv::key_selector::canonicalize_key_selector;

#[test]
fn uppercase_uuid_is_rewritten_to_canonical_form() {
    assert_eq!(
        canonicalize_key_selector("key:0B9F5D0E-5C1A-4A52-9A6E-6F1D7B0C1A11"),
        "key:0b9f5d0e-5c1a-4a52-9a6e-6f1d7b0c1a11"
    );
}

#[test]
fn other_spellings_are_rewritten() {
    let want = "key:0b9f5d0e-5c1a-4a52-9a6e-6f1d7b0c1a11";
    assert_eq!(
        canonicalize_key_selector("key:0b9f5d0e5c1a4a529a6e6f1d7b0c1a11"),
        want
    );
    assert_eq!(
        canonicalize_key_selector("key:{0b9f5d0e-5c1a-4a52-9a6e-6f1d7b0c1a11}"),
        want
    );
}

#[test]
fn non_key_and_unparseable_selectors_are_unchanged() {
    for s in ["*", "user:a@b.c", "group:KEY", "key:nope", "key:"] {
        assert_eq!(canonicalize_key_selector(s), s);
    }
}
