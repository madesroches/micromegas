//! No-DB unit tests for `micromegas_auth::membership::MembershipProvider`, using a canned
//! `FakeGroupSnapshot` test double in place of `DbGroupsSource`.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use micromegas_auth::groups::{ADMINS_GROUP, GroupGraph};
use micromegas_auth::membership::{GroupSnapshot, MembershipProvider};
use micromegas_auth::types::{
    AuthContext, AuthProvider, AuthType, HttpRequestParts, ProviderUnavailable, RequestParts,
};
use std::sync::Arc;

fn ctx(email: Option<&str>) -> AuthContext {
    AuthContext {
        subject: "test-subject".to_string(),
        email: email.map(String::from),
        issuer: "test-issuer".to_string(),
        audience: None,
        expires_at: None,
        auth_type: AuthType::Oidc,
        allow_delegation: false,
        bound_audience: None,
        read_audiences: vec![],
        memberships: Arc::from([]),
    }
}

fn parts() -> HttpRequestParts {
    HttpRequestParts {
        headers: http::HeaderMap::new(),
        method: http::Method::GET,
        uri: "/".parse().expect("valid uri"),
    }
}

struct FakeInner {
    result: std::sync::Mutex<Option<Result<AuthContext>>>,
}

#[async_trait]
impl AuthProvider for FakeInner {
    async fn validate_request(&self, _parts: &dyn RequestParts) -> Result<AuthContext> {
        self.result
            .lock()
            .expect("lock")
            .take()
            .expect("validate_request called more than once")
    }
}

enum FakeGroupSnapshot {
    Ok(GroupGraph),
    Err,
}

#[async_trait]
impl GroupSnapshot for FakeGroupSnapshot {
    async fn current(&self) -> Result<Arc<GroupGraph>> {
        match self {
            FakeGroupSnapshot::Ok(graph) => Ok(Arc::new(graph.clone())),
            FakeGroupSnapshot::Err => Err(ProviderUnavailable(anyhow!("group store down")).into()),
        }
    }
}

fn admins_wildcard_graph() -> GroupGraph {
    GroupGraph::from_rows(
        vec![ADMINS_GROUP.to_string()],
        vec![(ADMINS_GROUP.to_string(), "*".to_string())],
    )
    .expect("valid rows")
}

#[tokio::test]
async fn wrapper_fills_memberships_and_is_admin() {
    let inner = Arc::new(FakeInner {
        result: std::sync::Mutex::new(Some(Ok(ctx(None)))),
    });
    let groups: Arc<dyn GroupSnapshot> = Arc::new(FakeGroupSnapshot::Ok(admins_wildcard_graph()));
    let provider = MembershipProvider::new(inner, groups);

    let out = provider
        .validate_request(&parts() as &dyn RequestParts)
        .await
        .expect("validate_request");
    assert_eq!(out.memberships.as_ref(), &[ADMINS_GROUP.to_string()]);
    assert!(out.is_admin());
}

#[tokio::test]
async fn inner_err_passes_through_unchanged() {
    let inner = Arc::new(FakeInner {
        result: std::sync::Mutex::new(Some(Err(anyhow!("bad credential")))),
    });
    let groups: Arc<dyn GroupSnapshot> = Arc::new(FakeGroupSnapshot::Ok(admins_wildcard_graph()));
    let provider = MembershipProvider::new(inner, groups);

    let err = provider
        .validate_request(&parts() as &dyn RequestParts)
        .await
        .expect_err("inner error must propagate");
    assert_eq!(err.to_string(), "bad credential");
}

#[tokio::test]
async fn store_provider_unavailable_propagates_as_provider_unavailable() {
    let inner = Arc::new(FakeInner {
        result: std::sync::Mutex::new(Some(Ok(ctx(Some("alice@example.com"))))),
    });
    let groups: Arc<dyn GroupSnapshot> = Arc::new(FakeGroupSnapshot::Err);
    let provider = MembershipProvider::new(inner, groups);

    let err = provider
        .validate_request(&parts() as &dyn RequestParts)
        .await
        .expect_err("store outage must propagate");
    assert!(
        err.downcast_ref::<ProviderUnavailable>().is_some(),
        "expected ProviderUnavailable, got: {err:?}"
    );
}
