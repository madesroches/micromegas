//! Wraps an inner [`AuthProvider`] and resolves the caller's transitive local-group membership
//! once per request -- the one resolution site upstream of every consumer (`is_admin()`,
//! `caller_selectors`/`selector_matches`'s `group:` arm).

use crate::groups::{DbGroupsSource, GroupGraph};
use crate::types::{AuthContext, AuthProvider, RequestParts};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// The group-snapshot seam [`MembershipProvider`] resolves through -- implemented by
/// [`DbGroupsSource`] in production, and by a canned/error-returning test double in
/// `rust/auth/tests/membership_tests.rs`, so this crate's no-DB test policy can exercise the
/// wrapper's success and failure paths without a live Postgres.
#[async_trait]
pub trait GroupSnapshot: Send + Sync {
    /// Returns the current `GroupGraph` snapshot, or `Err` (typically a
    /// [`crate::types::ProviderUnavailable`]) on a store outage with no prior successful load.
    async fn current(&self) -> Result<Arc<GroupGraph>>;
}

#[async_trait]
impl GroupSnapshot for DbGroupsSource {
    async fn current(&self) -> Result<Arc<GroupGraph>> {
        DbGroupsSource::current(self).await
    }
}

/// Wraps `inner`, filling `AuthContext.memberships` with `groups.current()`'s closure over
/// `ctx.email` after `inner` authenticates the request.
pub struct MembershipProvider {
    inner: Arc<dyn AuthProvider>,
    groups: Arc<dyn GroupSnapshot>,
}

impl MembershipProvider {
    /// Wraps `inner`, resolving membership from `groups` on every `validate_request` call.
    pub fn new(inner: Arc<dyn AuthProvider>, groups: Arc<dyn GroupSnapshot>) -> Self {
        Self { inner, groups }
    }
}

#[async_trait]
impl AuthProvider for MembershipProvider {
    async fn validate_request(&self, parts: &dyn RequestParts) -> Result<AuthContext> {
        let mut ctx = self.inner.validate_request(parts).await?;
        // `ProviderUnavailable` propagates unchanged: a group-store outage denies the request
        // the same way an inner-provider store outage does.
        let graph = self.groups.current().await?;
        ctx.memberships = graph.closure(ctx.email.as_deref()).into();
        Ok(ctx)
    }
}
