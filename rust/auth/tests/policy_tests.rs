//! Unit tests for the authorization seam (#1369, AbAC Stage 1): `AudienceReadPolicy` and
//! `AudienceMintPolicy`.

use micromegas_auth::policy::{AudienceMintPolicy, AudienceReadPolicy, MintPolicy, ReadPolicy};
use micromegas_auth::types::{AuthContext, AuthType};

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

// ---------------------------------------------------------------------------
// AudienceReadPolicy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_policy_resolves_singleton_when_no_groups() {
    let policy = AudienceReadPolicy::new(vec![]);
    let ctx = caller(Some("alice@example.com"), vec![], vec![], false);
    let resolved = policy.resolve(&ctx).await.expect("resolve");
    assert_eq!(
        sorted(resolved.into_inner()),
        vec!["user:alice@example.com".to_string()]
    );
}

#[tokio::test]
async fn read_policy_resolves_union_of_identity_groups_and_implicit_groups() {
    let policy = AudienceReadPolicy::new(vec!["everyone".to_string()]);
    let ctx = caller(
        Some("alice@example.com"),
        vec!["team-a".to_string()],
        vec![],
        false,
    );
    let resolved = policy.resolve(&ctx).await.expect("resolve");
    assert_eq!(
        sorted(resolved.into_inner()),
        vec![
            "group:everyone".to_string(),
            "group:team-a".to_string(),
            "user:alice@example.com".to_string(),
        ]
    );
}

#[tokio::test]
async fn read_policy_every_element_is_prefixed() {
    let policy = AudienceReadPolicy::new(vec!["everyone".to_string()]);
    let ctx = caller(
        Some("alice@example.com"),
        vec!["team-a".to_string()],
        vec![],
        false,
    );
    let resolved = policy.resolve(&ctx).await.expect("resolve");
    for audience in resolved.into_inner().iter() {
        assert!(
            audience.starts_with("user:") || audience.starts_with("group:"),
            "audience {audience:?} is not user:/group:-prefixed"
        );
    }
}

#[tokio::test]
async fn read_policy_resolves_empty_set_for_a_caller_with_no_grants() {
    // No email, no groups claim, no implicit groups, no read_audiences grant -- an API key with
    // no Stage 4b grant in a privacy deployment. Must resolve to the empty set, never anything
    // permissive.
    let policy = AudienceReadPolicy::new(vec![]);
    let ctx = caller(None, vec![], vec![], false);
    let resolved = policy.resolve(&ctx).await.expect("resolve");
    assert!(
        resolved.into_inner().is_empty(),
        "expected the empty set for a grantless caller"
    );
}

#[tokio::test]
async fn read_policy_service_account_grant_has_no_user_element() {
    let policy = AudienceReadPolicy::new(vec!["everyone".to_string()]);
    let ctx = caller(
        None,
        vec![],
        vec![
            "group:analytics-a".to_string(),
            "group:analytics-b".to_string(),
        ],
        false,
    );
    let resolved = policy.resolve(&ctx).await.expect("resolve");
    assert_eq!(
        sorted(resolved.into_inner()),
        vec![
            "group:analytics-a".to_string(),
            "group:analytics-b".to_string(),
            "group:everyone".to_string(),
        ]
    );
}

/// An unconfigured deployment (implicit-groups env var unset) still resolves a scope -- the
/// caller's own singleton, not an error and not something permissive -- rather than leaving
/// `from_env`'s knob silently inert. Uses a prefix no other test/env touches, so "unset" holds
/// regardless of test execution order.
#[tokio::test]
async fn from_env_with_unset_var_resolves_the_caller_singleton() {
    let policy =
        AudienceReadPolicy::from_env("MICROMEGAS_1369_POLICY_TESTS_UNSET").expect("from_env");
    let ctx = caller(Some("alice@example.com"), vec![], vec![], false);
    let resolved = policy.resolve(&ctx).await.expect("resolve");
    assert_eq!(
        sorted(resolved.into_inner()),
        vec!["user:alice@example.com".to_string()]
    );
}

#[tokio::test]
async fn read_policy_service_account_with_empty_grant_resolves_implicit_only() {
    let policy = AudienceReadPolicy::new(vec!["everyone".to_string()]);
    let ctx = caller(None, vec![], vec![], false);
    let resolved = policy.resolve(&ctx).await.expect("resolve");
    assert_eq!(
        sorted(resolved.into_inner()),
        vec!["group:everyone".to_string()]
    );
}

// ---------------------------------------------------------------------------
// AudienceMintPolicy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mint_policy_defaults_to_user_email_when_no_requested_audience() {
    let policy = AudienceMintPolicy::new(vec![]);
    let ctx = caller(Some("alice@example.com"), vec![], vec![], false);
    let audience = policy
        .resolve_audience(&ctx, None)
        .await
        .expect("resolve_audience");
    assert_eq!(audience, "user:alice@example.com");
}

#[tokio::test]
async fn mint_policy_permits_a_requested_audience_inside_the_mintable_set() {
    let policy = AudienceMintPolicy::new(vec!["everyone".to_string()]);
    let ctx = caller(
        Some("alice@example.com"),
        vec!["team-a".to_string()],
        vec![],
        false,
    );
    for requested in ["user:alice@example.com", "group:team-a", "group:everyone"] {
        let audience = policy
            .resolve_audience(&ctx, Some(requested))
            .await
            .unwrap_or_else(|e| panic!("expected {requested} to be mintable, got {e}"));
        assert_eq!(audience, requested);
    }
}

#[tokio::test]
async fn mint_policy_rejects_a_requested_audience_outside_the_mintable_set() {
    let policy = AudienceMintPolicy::new(vec!["everyone".to_string()]);
    let ctx = caller(
        Some("alice@example.com"),
        vec!["team-a".to_string()],
        vec![],
        false,
    );
    let result = policy.resolve_audience(&ctx, Some("group:team-b")).await;
    assert!(result.is_err(), "expected group:team-b to be refused");
}

#[tokio::test]
async fn mint_policy_read_grant_confers_no_mint_authority() {
    let policy = AudienceMintPolicy::new(vec![]);
    // A service-account-shaped caller: no email, no groups claim, but a Stage 4b read grant for
    // "group:reporting". The read grant must not translate into mint authority for that
    // audience.
    let ctx = caller(None, vec![], vec!["group:reporting".to_string()], false);
    let result = policy.resolve_audience(&ctx, Some("group:reporting")).await;
    assert!(
        result.is_err(),
        "a read grant must not confer mint authority"
    );
}

#[tokio::test]
async fn mint_policy_admin_arm_permits_an_arbitrary_well_formed_audience() {
    let admin = caller(Some("admin@example.com"), vec![], vec![], true);
    let non_admin = caller(Some("alice@example.com"), vec![], vec![], false);
    let policy = AudienceMintPolicy::new(vec![]);

    // Another user's audience: refused for the non-admin caller...
    let non_admin_result = policy
        .resolve_audience(&non_admin, Some("user:bob@example.com"))
        .await;
    assert!(non_admin_result.is_err());

    // ...but permitted for the admin caller.
    let admin_result = policy
        .resolve_audience(&admin, Some("user:bob@example.com"))
        .await
        .expect("admin arm should permit an arbitrary well-formed audience");
    assert_eq!(admin_result, "user:bob@example.com");
}

#[tokio::test]
async fn mint_policy_rejects_a_malformed_audience_for_admin_and_non_admin() {
    let admin = caller(Some("admin@example.com"), vec![], vec![], true);
    let non_admin = caller(Some("alice@example.com"), vec![], vec![], false);
    let policy = AudienceMintPolicy::new(vec![]);

    for ctx in [&admin, &non_admin] {
        let result = policy
            .resolve_audience(ctx, Some("not-a-well-formed-audience"))
            .await;
        assert!(
            result.is_err(),
            "expected a malformed audience to be refused"
        );
    }
}
