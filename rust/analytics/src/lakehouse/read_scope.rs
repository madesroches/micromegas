//! The analytics-side half of the authorization seam (#1369, AbAC Stage 1).
//!
//! `ReadScope` is the query planner's authorization input -- what a `ReadPolicy`
//! (`micromegas-auth`) resolves down to. `micromegas-analytics` does not depend on
//! `micromegas-auth` (see `tasks/1369_policy_seam_plan.md` §1: that edge would pull in the whole
//! OIDC/JWT dependency tree behind one enum), so `ReadScope` carries resolved audiences only --
//! no group vocabulary, no policy trait, nothing that requires a store. The `rust/public` bridge
//! is the only place a `ReadableAudiences` (from `micromegas-auth`) becomes a `ReadScope`.
//!
//! **Stage 1 ships no enforcement.** Nothing in this crate consumes `ReadScope` yet -- Stage 2's
//! `OwnershipRewrite` and Stage 3's UDTF guards are the first consumers. Today `ReadScope` is
//! threaded down to `make_session_context` and stored, unread.

use std::sync::Arc;

/// The authorization scope a query planner may read under.
///
/// `All` is deliberately not something a `ReadPolicy` can produce (see
/// `micromegas_auth::policy::ReadPolicy`'s doc comment) -- it is the marker internal
/// (non-request) callers pass, via [`CallerContext::internal`] / [`CallerContext::maintenance`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadScope {
    /// No audience restriction. Only ever constructed by an internal/maintenance caller, or by
    /// the `rust/public` resolver when no `AuthContext` extension is present at all (no auth
    /// provider configured, e.g. `--disable-auth`) -- never as the output of resolving a
    /// `ReadPolicy` against a real caller.
    All,
    /// Restricted to the given set of audiences (`"user:<email>"` / `"group:<id>"`), as resolved
    /// by a `ReadPolicy`.
    Audiences(Arc<[String]>),
}

/// Bundles the two orthogonal authorization inputs `make_session_context` and friends need --
/// audience scope (`read_scope`) and the `is_admin` mutating-function-registration capability
/// (#1376/#1377) -- into one struct instead of two adjacent, transposable positional parameters.
///
/// Required (not `Option`/defaulted) at every call site by design: a defaulting parameter would
/// let a future call site inherit `ReadScope::All` by omission, which is exactly the failure this
/// seam exists to prevent. Every call site must state its scope explicitly, via one of the three
/// constructors below or a resolved value from a `ReadPolicy`.
#[derive(Debug, Clone)]
pub struct CallerContext {
    /// The audience scope to plan queries under. Stored/ignored by `make_session_context` in
    /// Stage 1 -- Stage 2/3 are the first consumers.
    pub read_scope: ReadScope,
    /// Whether the caller may use the five mutating lakehouse UDTFs/UDFs (unchanged from
    /// today's `is_admin: bool` parameter).
    pub is_admin: bool,
}

impl CallerContext {
    /// For background/materialization callers that are not serving a user request at all
    /// (`is_admin: false`, `ReadScope::All`). Distinct from [`Self::maintenance`] only in
    /// `is_admin` -- use this for internal call sites that must not register the mutating
    /// UDTFs/UDFs.
    pub fn internal() -> Self {
        Self {
            read_scope: ReadScope::All,
            is_admin: false,
        }
    }

    /// For background/materialization callers performing maintenance work (`is_admin: true`,
    /// `ReadScope::All`) -- never a user session.
    pub fn maintenance() -> Self {
        Self {
            read_scope: ReadScope::All,
            is_admin: true,
        }
    }
}
