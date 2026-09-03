//! The analytics-side half of the authorization seam.
//!
//! `ReadScope` is the query planner's authorization input -- what a `ReadPolicy`
//! (`micromegas-auth`) resolves down to. `micromegas-analytics` does not depend on
//! `micromegas-auth`: that edge would pull in the whole OIDC/JWT dependency tree behind one enum,
//! so `ReadScope` carries resolved audiences only -- no group vocabulary, no policy trait, nothing
//! that requires a store. The `rust/public` bridge is the only place a `ReadableAudiences` (from
//! `micromegas-auth`) becomes a `ReadScope`.
//!
//! Enforcement has two layers. [`super::ownership_rewrite::OwnershipRewrite`] (the row-level filter) reads
//! `ReadScope` out of [`CallerContext`] inside `query.rs::make_session_context` and injects an
//! audience predicate into every `MaterializedView`-backed scan. The call-level guard is the UDTF/UDF guards
//! ([`super::audience_guard::AudienceGuard`]) for the span/metadata functions the row-level filter structurally
//! cannot reach, plus `CallerContext.is_admin`'s mutating-function registration
//! gate. `view_instance(...)` is also guarded by the call-level guard, closing a cost/availability residual for
//! the six view sets carrying a physical `audience` column, where the row-level filter already filters
//! `view_instance` scans row-by-row, same as the named-table form; for the other five view sets,
//! reachable only through a guarded `view_instance(...)`, the call-level guard is the sole enforcement
//! -- the row-level filter injects no predicate there at all.

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
    /// Restricted to the given set of audiences -- opaque labels (e.g. `"public"`,
    /// `"team-alpha"`), as resolved by a `ReadPolicy`.
    Audiences(Arc<[String]>),
}

/// Bundles the orthogonal authorization inputs `make_session_context` and friends need --
/// audience scope (`read_scope`) and the `is_admin` mutating-function-registration capability --
/// into one struct instead of adjacent, transposable positional parameters.
///
/// Required (not `Option`/defaulted) at every call site by design: a defaulting parameter would
/// let a future call site inherit `ReadScope::All` by omission, which is exactly the failure this
/// seam exists to prevent. Every call site must state its scope explicitly, via one of the three
/// constructors below or a resolved value from a `ReadPolicy`.
#[derive(Debug, Clone)]
pub struct CallerContext {
    /// The audience scope to plan queries under. Consumed by
    /// [`super::ownership_rewrite::OwnershipRewrite`] inside `query.rs::make_session_context`.
    pub read_scope: ReadScope,
    /// Whether the caller may use the five mutating lakehouse UDTFs/UDFs.
    pub is_admin: bool,
    /// Per-service data-isolation deployment config (`MICROMEGAS_PUBLIC_VIEW_SETS`) -- resolved
    /// once at server startup, not per request, but bundled here rather than as a new
    /// `make_session_context` parameter: per-request resolved values ride the context,
    /// per-service objects live on the service, and this rides along with `read_scope` at every
    /// real call site anyway.
    pub isolation_config: Arc<IsolationConfig>,
    /// The caller's identity, as recorded in `deny_queries`'s `created_by` column. `None` on
    /// internal/maintenance paths -- such a caller cannot call `deny_queries`, which requires
    /// `Some`. One string, not a struct: `created_by` is the only consumer, so a richer identity
    /// type would be written at every construction site and read nowhere.
    pub identity: Option<String>,
    /// The grant selectors this caller matches -- `"*"`, `"user:<email>"` when an email is present,
    /// and one `"group:<g>"` per claimed group -- precomputed by `rust/public` from the
    /// `AuthContext` (`micromegas_auth::policy::caller_selectors`), so `micromegas-analytics` never
    /// needs the auth crate. Empty for internal and maintenance callers and for a request with no
    /// `AuthContext` at all (`--disable-auth`). Consumed by `list_audience_grants()`
    /// (`list_audience_grants_table_function.rs`); admins do not need it -- they see every row.
    pub grant_selectors: Arc<[String]>,
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
            isolation_config: Arc::new(IsolationConfig::default()),
            identity: None,
            grant_selectors: Arc::from([]),
        }
    }

    /// For background/materialization callers performing maintenance work (`is_admin: true`,
    /// `ReadScope::All`) -- never a user session.
    pub fn maintenance() -> Self {
        Self {
            read_scope: ReadScope::All,
            is_admin: true,
            isolation_config: Arc::new(IsolationConfig::default()),
            identity: None,
            grant_selectors: Arc::from([]),
        }
    }
}

/// Deployment config for the data-isolation seam: [`super::ownership_rewrite::OwnershipRewrite`]
/// (the row-level filter). Per-service, resolved once at server startup from environment variables -- see
/// [`IsolationConfig::from_env`].
///
/// `#[derive(Default)]`: `public_view_sets` is the only field, and its empty-by-default matches
/// [`Self::from_env`]'s own default.
#[derive(Debug, Clone, Default)]
pub struct IsolationConfig {
    /// View-set names `OwnershipRewrite` skips entirely -- no predicate injected at all,
    /// regardless of scope. Off (empty) by default, fail-closed for this
    /// operator-responsibility allowlist. Parsed from `MICROMEGAS_PUBLIC_VIEW_SETS`.
    pub public_view_sets: Vec<String>,
}

/// Comma-separated list parser for `MICROMEGAS_PUBLIC_VIEW_SETS`.
///
/// Deliberately a comma-separated list (rejecting `[`, `]`, `"`) rather than the
/// `MICROMEGAS_API_KEYS` JSON-array shape -- duplicated here rather than depending on
/// `micromegas-auth` for it, since `micromegas-analytics` does not depend on that crate (the same
/// crate-boundary reasoning `read_scope.rs`'s own module doc comment gives for keeping
/// `ReadScope` here).
fn parse_comma_separated_list(var: &str) -> anyhow::Result<Vec<String>> {
    let raw = match std::env::var(var) {
        Ok(raw) => raw,
        Err(_) => return Ok(vec![]),
    };
    if raw.trim().is_empty() {
        return Ok(vec![]);
    }
    let mut values = Vec::new();
    for entry in raw.split(',') {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            anyhow::bail!("{var}: empty entry (entry {entry:?} in {raw:?})");
        }
        if trimmed.contains(['[', ']', '"']) {
            anyhow::bail!(
                "{var}: entry {trimmed:?} contains '[', ']', or '\"' -- this variable is \
                 comma-separated, not a JSON array like MICROMEGAS_API_KEYS"
            );
        }
        values.push(trimmed.to_string());
    }
    Ok(values)
}

impl IsolationConfig {
    /// Resolves the surviving knob from the environment. `MICROMEGAS_PUBLIC_VIEW_SETS` unset ⇒
    /// no public view sets; a malformed entry is `Err`, not silently ignored -- a startup `?`
    /// turns a typo into a fail-fast instead of a silently-inert knob.
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            public_view_sets: parse_comma_separated_list("MICROMEGAS_PUBLIC_VIEW_SETS")?,
        })
    }
}
