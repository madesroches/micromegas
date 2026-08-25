//! Unit tests for the authorization seam (#1369, AbAC Stage 1; grant-map rewrite #1372, Stage
//! 4): `is_valid_audience`, `AudienceGrants`, `AudienceReadPolicy`, and `AudienceMintPolicy`.
//!
//! The `{prefix}_AUDIENCE_GRANTS`/`MICROMEGAS_AUDIENCE_GRANTS` fallback tests mutate
//! process-wide env vars, so they are `#[serial]` with an `EnvGuard` that restores them on
//! drop -- the same pattern as `rust/analytics/tests/ownership_rewrite_config_tests.rs`. A
//! test-only prefix keeps the *prefixed* var name from colliding with any other test/process
//! env.

use micromegas_auth::env::resolve_prefixed_var;
use micromegas_auth::policy::{
    AudienceGrants, AudienceMintPolicy, AudienceReadPolicy, GrantAxis, MintPolicy, PUBLIC_AUDIENCE,
    ReadPolicy, caller_selectors, default_audience_from_env, is_valid_audience, valid_selector,
};
use micromegas_auth::types::{AuthContext, AuthType};
use serial_test::serial;

/// Builds an `AuthContext` with sane defaults, overridden by the fields tests care about.
fn caller(
    email: Option<&str>,
    groups: Vec<String>,
    read_audiences: Vec<String>,
    is_admin: bool,
) -> AuthContext {
    AuthContext {
        subject: "test-subject".to_string(),
        email: email.map(String::from),
        issuer: "test-issuer".to_string(),
        audience: None,
        expires_at: None,
        auth_type: AuthType::Oidc,
        is_admin,
        allow_delegation: false,
        bound_audience: None,
        read_audiences,
        groups,
    }
}

fn sorted(audiences: std::sync::Arc<[String]>) -> Vec<String> {
    let mut v: Vec<String> = audiences.to_vec();
    v.sort();
    v
}

fn grants(json: &str) -> AudienceGrants {
    AudienceGrants::parse(json).expect("valid grant map")
}

// ---------------------------------------------------------------------------
// is_valid_audience
// ---------------------------------------------------------------------------

#[test]
fn is_valid_audience_accepts_opaque_labels() {
    for aud in ["public", "team-alpha", "Team_Alpha", "a", &"a".repeat(255)] {
        assert!(is_valid_audience(aud), "expected {aud:?} to be valid");
    }
}

#[test]
fn is_valid_audience_rejects_malformed_names() {
    for aud in [
        "",
        "alice@example.com",
        "team alpha",
        "a,b",
        "[\"x\"]",
        "it's",
        &"a".repeat(256),
    ] {
        assert!(!is_valid_audience(aud), "expected {aud:?} to be rejected");
    }
}

#[test]
fn is_valid_audience_does_not_normalize() {
    // Deliberately no case folding: "team-alpha" and "Team-Alpha" are two distinct audiences,
    // not the same bucket under different spellings.
    assert!(is_valid_audience("team-alpha"));
    assert!(is_valid_audience("Team-Alpha"));
    assert_ne!("team-alpha", "Team-Alpha");
}

// ---------------------------------------------------------------------------
// AudienceReadPolicy::resolve
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_policy_public_is_always_present() {
    let policy = AudienceReadPolicy::new(AudienceGrants::empty());
    let ctx = caller(None, vec![], vec![], false);
    let resolved = policy.resolve(&ctx).await.expect("resolve");
    assert!(resolved.into_inner().contains(&PUBLIC_AUDIENCE.to_string()));
}

#[tokio::test]
async fn read_policy_group_selector_matches_a_claim_value() {
    let policy = AudienceReadPolicy::new(grants(r#"{"team-alpha": ["group:eng"]}"#));
    let ctx = caller(None, vec!["eng".to_string()], vec![], false);
    let resolved = policy.resolve(&ctx).await.expect("resolve");
    assert!(sorted(resolved.into_inner()).contains(&"team-alpha".to_string()));
}

#[tokio::test]
async fn read_policy_user_selector_matches_the_email() {
    let policy = AudienceReadPolicy::new(grants(r#"{"team-alpha": ["user:alice@example.com"]}"#));
    let ctx = caller(Some("alice@example.com"), vec![], vec![], false);
    let resolved = policy.resolve(&ctx).await.expect("resolve");
    assert!(sorted(resolved.into_inner()).contains(&"team-alpha".to_string()));
}

#[tokio::test]
async fn read_policy_star_selector_grants_everyone() {
    let policy = AudienceReadPolicy::new(grants(r#"{"team-alpha": ["*"]}"#));
    let ctx = caller(None, vec![], vec![], false);
    let resolved = policy.resolve(&ctx).await.expect("resolve");
    assert!(sorted(resolved.into_inner()).contains(&"team-alpha".to_string()));
}

#[tokio::test]
async fn read_policy_audience_with_no_matching_selector_is_absent() {
    let policy = AudienceReadPolicy::new(grants(r#"{"team-alpha": ["group:eng"]}"#));
    let ctx = caller(None, vec!["sales".to_string()], vec![], false);
    let resolved = policy.resolve(&ctx).await.expect("resolve");
    assert!(!sorted(resolved.into_inner()).contains(&"team-alpha".to_string()));
}

/// There is no self-audience rule: a caller is granted no audience merely for being named like
/// one, including an API-key caller (whose `subject` is the key name) named after an audience
/// that grants it nothing.
#[tokio::test]
async fn read_policy_no_self_audience_rule() {
    let policy = AudienceReadPolicy::new(AudienceGrants::empty());
    let mut ctx = caller(None, vec![], vec![], false);
    ctx.subject = "team-alpha".to_string();
    let resolved = policy.resolve(&ctx).await.expect("resolve");
    assert!(
        !sorted(resolved.into_inner()).contains(&"team-alpha".to_string()),
        "a caller must not read an audience merely for being named like it"
    );
}

/// The fail-closed guarantee, restated for the grant-map model: a caller matching no selector
/// anywhere resolves to exactly `{public}` -- not a superset, not the empty set. Without this
/// exactness assertion, "public is always present" would pass just as well for a policy that
/// over-grants.
#[tokio::test]
async fn read_policy_grantless_caller_resolves_to_exactly_public() {
    let policy = AudienceReadPolicy::new(grants(r#"{"team-alpha": ["group:eng"]}"#));
    let ctx = caller(None, vec![], vec![], false);
    let resolved = policy.resolve(&ctx).await.expect("resolve");
    assert_eq!(
        sorted(resolved.into_inner()),
        vec![PUBLIC_AUDIENCE.to_string()]
    );
}

/// `read_audiences` (Stage 4b's per-key direct grant) still folds into the read axis, with no
/// `user:`-shaped element -- unlike the shipped identity-derived model, a service-account-shaped
/// caller (no email, no groups) contributes nothing but its direct grants plus `public`.
#[tokio::test]
async fn read_policy_read_audiences_folds_into_the_read_axis() {
    let policy = AudienceReadPolicy::new(AudienceGrants::empty());
    let ctx = caller(
        None,
        vec![],
        vec!["team-a".to_string(), "team-b".to_string()],
        false,
    );
    let resolved = policy.resolve(&ctx).await.expect("resolve");
    assert_eq!(
        sorted(resolved.into_inner()),
        vec![
            PUBLIC_AUDIENCE.to_string(),
            "team-a".to_string(),
            "team-b".to_string(),
        ]
    );
}

/// The motivating scenario end to end: data stamped `alice-laptop` is invisible to bob until an
/// operator edits one grant -- no data changes.
#[tokio::test]
async fn read_policy_editing_a_grant_changes_visibility_with_no_data_change() {
    let bob = caller(None, vec!["leads".to_string()], vec![], false);

    let before = AudienceReadPolicy::new(AudienceGrants::empty());
    let resolved = before.resolve(&bob).await.expect("resolve");
    assert!(!sorted(resolved.into_inner()).contains(&"alice-laptop".to_string()));

    let after = AudienceReadPolicy::new(grants(r#"{"alice-laptop": ["group:leads"]}"#));
    let resolved = after.resolve(&bob).await.expect("resolve");
    assert!(sorted(resolved.into_inner()).contains(&"alice-laptop".to_string()));
}

// ---------------------------------------------------------------------------
// AudienceMintPolicy::resolve_audience
// ---------------------------------------------------------------------------

/// `requested: None` is now always an error, admin or not -- there is no "myself" audience to
/// default to under the opaque-label model.
#[tokio::test]
async fn mint_policy_no_requested_audience_is_always_an_error() {
    let policy = AudienceMintPolicy::new(AudienceGrants::empty());
    for is_admin in [false, true] {
        let ctx = caller(Some("alice@example.com"), vec![], vec![], is_admin);
        let result = policy.resolve_audience(&ctx, None).await;
        assert!(
            result.is_err(),
            "expected requested: None to be refused (is_admin={is_admin})"
        );
    }
}

/// `read_audiences` never enters the mintable set: a service-account caller with a read grant
/// but no mint grant for the same audience must be refused.
#[tokio::test]
async fn mint_policy_read_audiences_never_enter_the_mintable_set() {
    let policy = AudienceMintPolicy::new(AudienceGrants::empty());
    let ctx = caller(None, vec![], vec!["team-a".to_string()], false);
    let result = policy.resolve_audience(&ctx, Some("team-a")).await;
    assert!(
        result.is_err(),
        "a read_audiences entry must not confer mint authority"
    );
}

/// `PUBLIC_AUDIENCE` is always in a non-admin's readable set but never in their mintable set
/// unless a grant explicitly names it in a `"mint"` list.
#[tokio::test]
async fn mint_policy_public_is_not_mintable_by_default() {
    let policy = AudienceMintPolicy::new(AudienceGrants::empty());
    let ctx = caller(Some("alice@example.com"), vec![], vec![], false);
    let result = policy.resolve_audience(&ctx, Some(PUBLIC_AUDIENCE)).await;
    assert!(
        result.is_err(),
        "public must not be mintable without an explicit mint grant"
    );
}

#[tokio::test]
async fn mint_policy_admin_may_mint_any_valid_audience_including_public() {
    let policy = AudienceMintPolicy::new(AudienceGrants::empty());
    let admin = caller(Some("admin@example.com"), vec![], vec![], true);
    for aud in [PUBLIC_AUDIENCE, "team-alpha", "anything-valid"] {
        let resolved = policy
            .resolve_audience(&admin, Some(aud))
            .await
            .unwrap_or_else(|e| panic!("expected admin to mint {aud:?}, got {e}"));
        assert_eq!(resolved, aud);
    }
}

#[tokio::test]
async fn mint_policy_admin_arm_rejects_a_malformed_audience() {
    let policy = AudienceMintPolicy::new(AudienceGrants::empty());
    let admin = caller(Some("admin@example.com"), vec![], vec![], true);
    for aud in ["not valid", "a:b"] {
        let result = policy.resolve_audience(&admin, Some(aud)).await;
        assert!(result.is_err(), "expected {aud:?} to be refused");
    }
}

/// A bare-array (read-only) grant confers no mint authority, even for a caller who matches its
/// read selector -- the central axis-independence property this stage's format change exists to
/// make testable.
#[tokio::test]
async fn mint_policy_a_read_only_grant_confers_no_mint_authority() {
    let policy = AudienceMintPolicy::new(grants(r#"{"team-alpha": ["group:eng"]}"#));
    let ctx = caller(None, vec!["eng".to_string()], vec![], false);
    let result = policy.resolve_audience(&ctx, Some("team-alpha")).await;
    assert!(
        result.is_err(),
        "a read-only (bare-array) grant must not confer mint authority"
    );
}

/// An explicit `"mint"` entry grants mint authority independent of `"read"`.
#[tokio::test]
async fn mint_policy_an_explicit_mint_entry_grants_authority() {
    let policy = AudienceMintPolicy::new(grants(
        r#"{"alice-laptop": {"read": ["group:leads"], "mint": ["user:alice@example.com"]}}"#,
    ));
    let ctx = caller(Some("alice@example.com"), vec![], vec![], false);
    let resolved = policy
        .resolve_audience(&ctx, Some("alice-laptop"))
        .await
        .expect("alice should be able to mint into her own audience");
    assert_eq!(resolved, "alice-laptop");
}

#[tokio::test]
async fn mint_policy_non_admin_rejects_an_audience_outside_the_mintable_set() {
    let policy = AudienceMintPolicy::new(grants(
        r#"{"alice-laptop": {"read": [], "mint": ["user:alice@example.com"]}}"#,
    ));
    let ctx = caller(Some("bob@example.com"), vec![], vec![], false);
    let result = policy.resolve_audience(&ctx, Some("alice-laptop")).await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// AudienceGrants::parse -- shape errors
// ---------------------------------------------------------------------------

#[test]
fn grants_parse_rejects_a_non_object_top_level() {
    assert!(AudienceGrants::parse(r#"["not", "an", "object"]"#).is_err());
}

#[test]
fn grants_parse_rejects_a_value_that_is_neither_bare_array_nor_object() {
    assert!(AudienceGrants::parse(r#"{"team-alpha": "not-an-array-or-object"}"#).is_err());
    assert!(AudienceGrants::parse(r#"{"team-alpha": 42}"#).is_err());
}

#[test]
fn grants_parse_rejects_a_non_array_read_or_mint_field() {
    assert!(AudienceGrants::parse(r#"{"team-alpha": {"read": "not-an-array"}}"#).is_err());
    assert!(AudienceGrants::parse(r#"{"team-alpha": {"read": [], "mint": "nope"}}"#).is_err());
}

#[test]
fn grants_parse_rejects_a_non_string_selector() {
    assert!(AudienceGrants::parse(r#"{"team-alpha": [42]}"#).is_err());
}

/// A misspelled key (`"raed"` for `"read"`) in the object form must fail startup, not parse
/// into an empty, silently-inert grant -- see the module doc comment on `RawGrantValue`.
#[test]
fn grants_parse_rejects_an_unknown_field() {
    assert!(AudienceGrants::parse(r#"{"team-alpha": {"raed": ["group:eng"]}}"#).is_err());
}

// ---------------------------------------------------------------------------
// AudienceGrants::parse -- content errors
// ---------------------------------------------------------------------------

#[test]
fn grants_parse_rejects_an_invalid_audience_key() {
    // The exact value operators are migrating *from* -- it must never silently match nothing.
    let err = AudienceGrants::parse(r#"{"group:everyone": ["*"]}"#)
        .expect_err("group:everyone is not a valid audience name");
    assert!(err.to_string().contains("group:everyone"));

    assert!(AudienceGrants::parse(r#"{"": ["*"]}"#).is_err());
}

#[test]
fn grants_parse_rejects_an_unrecognized_selector_prefix() {
    for selector in [
        r#"["eng"]"#,
        r#"["users:alice@example.com"]"#,
        r#"["group:"]"#,
    ] {
        let json = format!(r#"{{"team-alpha": {selector}}}"#);
        assert!(
            AudienceGrants::parse(&json).is_err(),
            "expected {json} to be rejected"
        );
    }
}

/// `serde_json`'s own `Map` deserialization would silently keep the *last* value for a
/// duplicate key -- `AudienceGrants::parse` must name it instead.
#[test]
fn grants_parse_rejects_a_duplicate_audience_key() {
    let err = AudienceGrants::parse(r#"{"team-alpha": ["group:a"], "team-alpha": ["group:b"]}"#)
        .expect_err("a duplicate key must be rejected, not silently resolved to the last value");
    assert!(err.to_string().contains("team-alpha"));
}

// ---------------------------------------------------------------------------
// valid_selector (#1489, AbAC Stage 6a: now `pub` for `analytics-web-srv`'s admin route)
// ---------------------------------------------------------------------------

#[test]
fn valid_selector_accepts_star_user_and_group() {
    for selector in ["*", "user:alice@example.com", "group:eng"] {
        assert!(
            valid_selector(selector),
            "expected {selector:?} to be valid"
        );
    }
}

#[test]
fn valid_selector_rejects_empty_or_unrecognized_prefixes() {
    for selector in ["", "user:", "group:", "users:alice", "eng"] {
        assert!(
            !valid_selector(selector),
            "expected {selector:?} to be rejected"
        );
    }
}

// ---------------------------------------------------------------------------
// caller_selectors (#1489/#1510, AbAC Stage 6a/6b) -- the shared builder for
// `CallerContext::grant_selectors`, `my_audiences`, and the write policy's hold check.
// ---------------------------------------------------------------------------

#[test]
fn caller_selectors_with_no_email_and_no_groups_is_just_star() {
    let c = caller(None, vec![], vec![], false);
    assert_eq!(caller_selectors(&c), vec!["*".to_string()]);
}

#[test]
fn caller_selectors_with_email_only() {
    let c = caller(Some("alice@example.com"), vec![], vec![], false);
    assert_eq!(
        caller_selectors(&c),
        vec!["*".to_string(), "user:alice@example.com".to_string()]
    );
}

#[test]
fn caller_selectors_with_groups_only() {
    let c = caller(
        None,
        vec!["eng".to_string(), "leads".to_string()],
        vec![],
        false,
    );
    assert_eq!(
        caller_selectors(&c),
        vec![
            "*".to_string(),
            "group:eng".to_string(),
            "group:leads".to_string(),
        ]
    );
}

#[test]
fn caller_selectors_with_both_email_and_groups() {
    let c = caller(
        Some("alice@example.com"),
        vec!["eng".to_string()],
        vec![],
        false,
    );
    assert_eq!(
        caller_selectors(&c),
        vec![
            "*".to_string(),
            "user:alice@example.com".to_string(),
            "group:eng".to_string(),
        ]
    );
}

// ---------------------------------------------------------------------------
// AudienceGrants::from_rows / merge (#1489, AbAC Stage 6a)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn from_rows_builds_read_and_mint_lists_from_axis() {
    let grants = AudienceGrants::from_rows([
        (
            "team-alpha".to_string(),
            GrantAxis::Read,
            "group:eng".to_string(),
        ),
        (
            "team-alpha".to_string(),
            GrantAxis::Mint,
            "user:alice@example.com".to_string(),
        ),
    ])
    .expect("valid rows");

    let read_policy = AudienceReadPolicy::new(grants.clone());
    let reader = caller(None, vec!["eng".to_string()], vec![], false);
    let resolved = read_policy.resolve(&reader).await.expect("resolve");
    assert!(sorted(resolved.into_inner()).contains(&"team-alpha".to_string()));

    let mint_policy = AudienceMintPolicy::new(grants);
    let minter = caller(Some("alice@example.com"), vec![], vec![], false);
    let resolved = mint_policy
        .resolve_audience(&minter, Some("team-alpha"))
        .await;
    assert_eq!(resolved.expect("alice can mint"), "team-alpha");
}

#[test]
fn from_rows_rejects_an_invalid_audience() {
    let err = AudienceGrants::from_rows([(
        "group:everyone".to_string(),
        GrantAxis::Read,
        "*".to_string(),
    )])
    .expect_err("group:everyone is not a valid audience name");
    assert!(err.to_string().contains("group:everyone"));
}

#[test]
fn from_rows_rejects_an_invalid_selector() {
    let err = AudienceGrants::from_rows([(
        "team-alpha".to_string(),
        GrantAxis::Read,
        "not-a-selector".to_string(),
    )])
    .expect_err("expected an invalid selector to be rejected");
    assert!(err.to_string().contains("not-a-selector"));
}

/// `AudienceGrants::merge` as a standalone utility (#1489, AbAC Stage 6a) -- **not** a test of
/// `resolve`'s runtime union behavior, which checks the env map and the DB store snapshot as two
/// separate sources and never calls `merge` (see the doc comment on `merge` itself). This test
/// merges two maps directly, then wraps the *single* merged result in one `AudienceReadPolicy` to
/// exercise it; a selector present in both input maps grants exactly the same access as being
/// present in either alone -- no dedup, no special-cased "duplicate" handling.
#[tokio::test]
async fn merge_unions_disjoint_and_overlapping_audiences() {
    let env_grants = grants(r#"{"team-alpha": ["group:eng"], "team-beta": ["*"]}"#);
    let store_grants = AudienceGrants::from_rows([
        (
            "team-alpha".to_string(),
            GrantAxis::Read,
            "group:eng".to_string(),
        ),
        (
            "team-gamma".to_string(),
            GrantAxis::Read,
            "user:carol@example.com".to_string(),
        ),
    ])
    .expect("valid rows");

    let merged = env_grants.merge(&store_grants);
    let policy = AudienceReadPolicy::new(merged);

    let eng_caller = caller(None, vec!["eng".to_string()], vec![], false);
    let resolved = sorted(
        policy
            .resolve(&eng_caller)
            .await
            .expect("resolve")
            .into_inner(),
    );
    assert!(resolved.contains(&"team-alpha".to_string()));

    let anyone = caller(None, vec![], vec![], false);
    let resolved = sorted(policy.resolve(&anyone).await.expect("resolve").into_inner());
    assert!(resolved.contains(&"team-beta".to_string()));

    let carol = caller(Some("carol@example.com"), vec![], vec![], false);
    let resolved = sorted(policy.resolve(&carol).await.expect("resolve").into_inner());
    assert!(resolved.contains(&"team-gamma".to_string()));
}

/// `AudienceGrants::merge` as a standalone utility (#1489, AbAC Stage 6a) -- again not a test of
/// `resolve`'s two-source runtime check (see the previous test's doc comment). A selector present
/// in *both* input maps for the same audience costs a redundant comparison when `merge`'s result
/// is later resolved, never a wrong answer -- still resolves, no error, no duplicate entry visible
/// in the resolved set (which is a `BTreeSet` regardless).
#[tokio::test]
async fn merge_tolerates_an_identical_selector_in_both_sources() {
    let env_grants = grants(r#"{"team-alpha": ["group:eng"]}"#);
    let store_grants = AudienceGrants::from_rows([(
        "team-alpha".to_string(),
        GrantAxis::Read,
        "group:eng".to_string(),
    )])
    .expect("valid rows");

    let merged = env_grants.merge(&store_grants);
    let policy = AudienceReadPolicy::new(merged);
    let eng_caller = caller(None, vec!["eng".to_string()], vec![], false);
    let resolved = policy.resolve(&eng_caller).await.expect("resolve");
    assert!(sorted(resolved.into_inner()).contains(&"team-alpha".to_string()));
}

// `AudienceReadPolicy::with_store`/`AudienceMintPolicy::with_store` (#1489, AbAC Stage 6a):
// store-outage/merge behavior is exercised in `rust/auth/tests/db_audience_grants_tests.rs`,
// which owns `DbAudienceGrantsSource` construction.

// ---------------------------------------------------------------------------
// {prefix}_AUDIENCE_GRANTS env fallback
// ---------------------------------------------------------------------------

const PREFIX: &str = "MICROMEGAS_1372_POLICY_TESTS";
const PREFIXED_VAR: &str = "MICROMEGAS_1372_POLICY_TESTS_AUDIENCE_GRANTS";
const UNPREFIXED_VAR: &str = "MICROMEGAS_AUDIENCE_GRANTS";

/// Clears both vars on drop so a failing assertion in one test can't leak state into the next.
struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: tests are serialized with `#[serial]`.
        unsafe {
            std::env::remove_var(PREFIXED_VAR);
            std::env::remove_var(UNPREFIXED_VAR);
        }
    }
}

/// An unconfigured deployment (`{prefix}_AUDIENCE_GRANTS` unset) still resolves a scope -- the
/// `{public}` singleton, not an error and not something permissive -- rather than leaving the
/// knob silently inert. Uses a prefix no other test/env touches, so "unset" holds regardless of
/// test execution order.
#[tokio::test]
#[serial]
async fn from_env_with_unset_var_resolves_to_public_only() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(PREFIXED_VAR);
        std::env::remove_var(UNPREFIXED_VAR);
    }
    let policy = AudienceReadPolicy::from_env(PREFIX).expect("from_env");
    let ctx = caller(Some("alice@example.com"), vec![], vec![], false);
    let resolved = policy.resolve(&ctx).await.expect("resolve");
    assert_eq!(
        sorted(resolved.into_inner()),
        vec![PUBLIC_AUDIENCE.to_string()]
    );
}

#[tokio::test]
#[serial]
async fn from_env_reads_the_unprefixed_fallback_when_prefixed_is_unset() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(PREFIXED_VAR);
        std::env::set_var(UNPREFIXED_VAR, r#"{"team-alpha": ["*"]}"#);
    }
    let policy = AudienceReadPolicy::from_env(PREFIX).expect("from_env");
    let ctx = caller(None, vec![], vec![], false);
    let resolved = policy.resolve(&ctx).await.expect("resolve");
    assert!(sorted(resolved.into_inner()).contains(&"team-alpha".to_string()));
}

#[tokio::test]
#[serial]
async fn from_env_prefixed_var_wins_over_unprefixed_fallback() {
    let _guard = EnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(PREFIXED_VAR, r#"{"prefixed": ["*"]}"#);
        std::env::set_var(UNPREFIXED_VAR, r#"{"unprefixed": ["*"]}"#);
    }
    let policy = AudienceReadPolicy::from_env(PREFIX).expect("from_env");
    let ctx = caller(None, vec![], vec![], false);
    let resolved = policy.resolve(&ctx).await.expect("resolve");
    let resolved = sorted(resolved.into_inner());
    assert!(resolved.contains(&"prefixed".to_string()));
    assert!(!resolved.contains(&"unprefixed".to_string()));
}

// ---------------------------------------------------------------------------
// default_audience_from_env
// ---------------------------------------------------------------------------

const DKA_PREFIX: &str = "MICROMEGAS_1372_POLICY_TESTS";
const DKA_PREFIXED_VAR: &str = "MICROMEGAS_1372_POLICY_TESTS_DEFAULT_AUDIENCE";
const DKA_UNPREFIXED_VAR: &str = "MICROMEGAS_DEFAULT_AUDIENCE";

/// Clears both vars on drop so a failing assertion in one test can't leak state into the next.
struct DefaultKeyAudienceEnvGuard;

impl Drop for DefaultKeyAudienceEnvGuard {
    fn drop(&mut self) {
        // SAFETY: tests are serialized with `#[serial]`.
        unsafe {
            std::env::remove_var(DKA_PREFIXED_VAR);
            std::env::remove_var(DKA_UNPREFIXED_VAR);
        }
    }
}

/// Unset resolves to `public`, the one built-in read grant every authenticated principal holds --
/// there is no "no default configured" state left to represent (#1482): one knob, always a value.
#[test]
#[serial]
fn default_audience_from_env_neither_var_set_is_public() {
    let _guard = DefaultKeyAudienceEnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(DKA_PREFIXED_VAR);
        std::env::remove_var(DKA_UNPREFIXED_VAR);
    }
    assert_eq!(
        default_audience_from_env(DKA_PREFIX).expect("from_env"),
        PUBLIC_AUDIENCE
    );
}

/// An empty or whitespace-only value is now a validation failure, not "unset": with one knob
/// carrying one meaning, an operator who sets it to empty has misconfigured the deployment rather
/// than opted out of a feature, and silently substituting `public` would be the wrong guess to
/// make about which audience their data belongs to.
#[test]
#[serial]
fn default_audience_from_env_empty_value_is_an_error() {
    let _guard = DefaultKeyAudienceEnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(DKA_PREFIXED_VAR);
        std::env::set_var(DKA_UNPREFIXED_VAR, "   ");
    }
    assert!(default_audience_from_env(DKA_PREFIX).is_err());
}

#[test]
#[serial]
fn default_audience_from_env_invalid_value_is_an_error() {
    let _guard = DefaultKeyAudienceEnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(DKA_PREFIXED_VAR);
        std::env::set_var(DKA_UNPREFIXED_VAR, "not valid");
    }
    assert!(default_audience_from_env(DKA_PREFIX).is_err());
}

#[test]
#[serial]
fn default_audience_from_env_valid_value_is_returned() {
    let _guard = DefaultKeyAudienceEnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(DKA_PREFIXED_VAR);
        std::env::set_var(DKA_UNPREFIXED_VAR, "team-alpha");
    }
    assert_eq!(
        default_audience_from_env(DKA_PREFIX).expect("from_env"),
        "team-alpha"
    );
}

/// A whitespace-padded value is trimmed before validation and storage -- matching
/// `IsolationConfig::from_env`'s trim-then-validate order in `read_scope.rs`. The trim is
/// `warn!`-logged (not asserted here, since the log sink is process-wide): the ingestion role
/// reads this same variable without trimming and would reject the padded form at startup.
#[test]
#[serial]
fn default_audience_from_env_whitespace_padded_value_is_trimmed() {
    let _guard = DefaultKeyAudienceEnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(DKA_PREFIXED_VAR);
        std::env::set_var(DKA_UNPREFIXED_VAR, "  team-alpha  ");
    }
    assert_eq!(
        default_audience_from_env(DKA_PREFIX).expect("from_env"),
        "team-alpha"
    );
}

#[test]
#[serial]
fn default_audience_from_env_prefixed_var_wins_over_unprefixed_fallback() {
    let _guard = DefaultKeyAudienceEnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(DKA_PREFIXED_VAR, "prefixed-audience");
        std::env::set_var(DKA_UNPREFIXED_VAR, "unprefixed-audience");
    }
    assert_eq!(
        default_audience_from_env(DKA_PREFIX).expect("from_env"),
        "prefixed-audience"
    );
}

// ---------------------------------------------------------------------------
// resolve_prefixed_var (`rust/auth/src/env.rs`) -- shared by every `{prefix}_*`-with-fallback
// knob in this crate. Env-mutating, so `#[serial]` like its neighbours above.
// ---------------------------------------------------------------------------

const RPV_SUFFIX: &str = "1373_POLICY_TESTS_KNOB";
const RPV_PREFIX: &str = "MICROMEGAS_1373A";
const RPV_PREFIXED_VAR: &str = "MICROMEGAS_1373A_1373_POLICY_TESTS_KNOB";
const RPV_UNPREFIXED_VAR: &str = "MICROMEGAS_1373_POLICY_TESTS_KNOB";

/// Clears both vars on drop so a failing assertion in one test can't leak state into the next.
struct ResolvePrefixedVarEnvGuard;

impl Drop for ResolvePrefixedVarEnvGuard {
    fn drop(&mut self) {
        // SAFETY: tests are serialized with `#[serial]`.
        unsafe {
            std::env::remove_var(RPV_PREFIXED_VAR);
            std::env::remove_var(RPV_UNPREFIXED_VAR);
        }
    }
}

#[test]
#[serial]
fn resolve_prefixed_var_empty_prefix_always_resolves_unprefixed() {
    let _guard = ResolvePrefixedVarEnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(RPV_PREFIXED_VAR, "irrelevant");
    }
    assert_eq!(resolve_prefixed_var("", RPV_SUFFIX), RPV_UNPREFIXED_VAR);
}

#[test]
#[serial]
fn resolve_prefixed_var_falls_back_to_unprefixed_when_prefixed_unset() {
    let _guard = ResolvePrefixedVarEnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::remove_var(RPV_PREFIXED_VAR);
        std::env::remove_var(RPV_UNPREFIXED_VAR);
    }
    assert_eq!(
        resolve_prefixed_var(RPV_PREFIX, RPV_SUFFIX),
        RPV_UNPREFIXED_VAR
    );
}

#[test]
#[serial]
fn resolve_prefixed_var_prefixed_wins_when_set() {
    let _guard = ResolvePrefixedVarEnvGuard;
    // SAFETY: serialized via `#[serial]`.
    unsafe {
        std::env::set_var(RPV_PREFIXED_VAR, "anything");
        std::env::remove_var(RPV_UNPREFIXED_VAR);
    }
    assert_eq!(
        resolve_prefixed_var(RPV_PREFIX, RPV_SUFFIX),
        RPV_PREFIXED_VAR
    );
}
