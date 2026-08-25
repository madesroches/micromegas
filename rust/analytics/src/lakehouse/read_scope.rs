//! The analytics-side half of the authorization seam (#1369, AbAC Stage 1).
//!
//! `ReadScope` is the query planner's authorization input -- what a `ReadPolicy`
//! (`micromegas-auth`) resolves down to. `micromegas-analytics` does not depend on
//! `micromegas-auth` (see `tasks/1369_policy_seam_plan.md` §1: that edge would pull in the whole
//! OIDC/JWT dependency tree behind one enum), so `ReadScope` carries resolved audiences only --
//! no group vocabulary, no policy trait, nothing that requires a store. The `rust/public` bridge
//! is the only place a `ReadableAudiences` (from `micromegas-auth`) becomes a `ReadScope`.
//!
//! **Stage 2 (#1370) consumes `ReadScope`.** [`super::ownership_rewrite::OwnershipRewrite`] --
//! Prong A of the two-pronged enforcement design -- reads it out of [`CallerContext`] inside
//! `query.rs::make_session_context` and injects an audience predicate into every
//! `MaterializedView`-backed scan. **Stage 3 (#1371) adds Prong B**: the UDTF/UDF guards
//! ([`super::audience_guard::AudienceGuard`]) for the span/metadata functions Prong A
//! structurally cannot reach, plus [`CallerContext::admin_principal_possible`]'s mutating-function
//! registration gate.

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
/// audience scope (`read_scope`) and the `is_admin`/`admin_principal_possible`
/// mutating-function-registration capability (#1376/#1377, #1371) -- into one struct instead of
/// adjacent, transposable positional parameters.
///
/// Required (not `Option`/defaulted) at every call site by design: a defaulting parameter would
/// let a future call site inherit `ReadScope::All` by omission, which is exactly the failure this
/// seam exists to prevent. Every call site must state its scope explicitly, via one of the three
/// constructors below or a resolved value from a `ReadPolicy`.
#[derive(Debug, Clone)]
pub struct CallerContext {
    /// The audience scope to plan queries under. Consumed by
    /// [`super::ownership_rewrite::OwnershipRewrite`] (#1370, AbAC Stage 2) inside
    /// `query.rs::make_session_context`.
    pub read_scope: ReadScope,
    /// Whether the caller may use the five mutating lakehouse UDTFs/UDFs (unchanged from
    /// today's `is_admin: bool` parameter).
    pub is_admin: bool,
    /// Per-service data-isolation deployment config (`MICROMEGAS_PUBLIC_VIEW_SETS`) -- resolved
    /// once at server startup, not per request, but
    /// bundled here rather than as a new `make_session_context` parameter (#1370 Design §8):
    /// per-request resolved values ride the context, per-service objects live on the service,
    /// and this rides along with `read_scope` at every real call site anyway.
    pub isolation_config: Arc<IsolationConfig>,
    /// Whether this *deployment* -- not this caller -- can ever produce an admin principal at
    /// all, derived once at startup from `AuthProvider::can_grant_admin`
    /// (`rust/public/src/servers/flight_sql_server.rs`) and copied onto every `CallerContext`
    /// unchanged, the same treatment `isolation_config` gets. Consumed by `query.rs`'s mutating
    /// UDTF/UDF registration gate (#1371, AbAC Stage 3, Prong B):
    /// `caller.is_admin || !caller.admin_principal_possible`. Named for the fact it represents
    /// (can this deployment ever produce an admin?), not its effect -- when `false` (an
    /// API-key-only deployment, which can never mint an admin), the mutating functions are
    /// registered for any authenticated caller rather than staying admin-only, since otherwise
    /// they would be unreachable by anyone.
    pub admin_principal_possible: bool,
    /// The caller's identity, as recorded in `deny_queries`'s `created_by` column
    /// (`tasks/query_deny_list_plan.md` §1/§7). `None` on internal/maintenance paths -- such a
    /// caller cannot call `deny_queries`, which requires `Some` (§8). One string, not a struct:
    /// `created_by` is the only consumer anywhere in that plan, so a richer identity type would
    /// be written at every construction site and read nowhere.
    pub identity: Option<String>,
    /// The grant selectors this caller matches -- `"*"`, `"user:<email>"` when an email is present,
    /// and one `"group:<g>"` per claimed group -- precomputed by `rust/public` from the
    /// `AuthContext` (`micromegas_auth::policy::caller_selectors`), so `micromegas-analytics` never
    /// needs the auth crate. Empty for internal and maintenance callers and for a request with no
    /// `AuthContext` at all (`--disable-auth`). Consumed by `list_audience_grants()`
    /// (`list_audience_grants_table_function.rs`, #1489, AbAC Stage 6b); admins do not need it --
    /// they see every row.
    pub grant_selectors: Arc<[String]>,
}

impl CallerContext {
    /// For background/materialization callers that are not serving a user request at all
    /// (`is_admin: false`, `ReadScope::All`). Distinct from [`Self::maintenance`] only in
    /// `is_admin` -- use this for internal call sites that must not register the mutating
    /// UDTFs/UDFs. `admin_principal_possible: true` so the gate's fallback (any-caller
    /// registration when a deployment has no admin principal) never fires for this
    /// non-user-request caller -- it is `is_admin` alone that must decide, same as today.
    pub fn internal() -> Self {
        Self {
            read_scope: ReadScope::All,
            is_admin: false,
            isolation_config: Arc::new(IsolationConfig::default()),
            admin_principal_possible: true,
            identity: None,
            grant_selectors: Arc::from([]),
        }
    }

    /// For background/materialization callers performing maintenance work (`is_admin: true`,
    /// `ReadScope::All`) -- never a user session. `admin_principal_possible`'s value is moot here
    /// (the gate's `caller.is_admin` arm already passes), kept `true` for consistency with
    /// [`Self::internal`].
    pub fn maintenance() -> Self {
        Self {
            read_scope: ReadScope::All,
            is_admin: true,
            isolation_config: Arc::new(IsolationConfig::default()),
            admin_principal_possible: true,
            identity: None,
            grant_selectors: Arc::from([]),
        }
    }
}

/// Deployment config for the data-isolation seam: [`super::ownership_rewrite::OwnershipRewrite`]
/// (#1370, AbAC Stage 2, Prong A). Per-service, resolved once at server startup from environment
/// variables -- see [`IsolationConfig::from_env`].
///
/// `#[derive(Default)]`: `public_view_sets` is the only field, and its empty-by-default matches
/// [`Self::from_env`]'s own default.
#[derive(Debug, Clone, Default)]
pub struct IsolationConfig {
    /// View-set names `OwnershipRewrite` skips entirely -- no predicate injected at all,
    /// regardless of scope. Off (empty) by default, matching the AbAC plan's "off by default,
    /// fail-closed" framing for this operator-responsibility allowlist. Parsed from
    /// `{prefix}_PUBLIC_VIEW_SETS`, falling back to `MICROMEGAS_PUBLIC_VIEW_SETS`.
    pub public_view_sets: Vec<String>,
}

/// Comma-separated list parser for `{prefix}_PUBLIC_VIEW_SETS` / `MICROMEGAS_PUBLIC_VIEW_SETS`.
///
/// Deliberately a comma-separated list (rejecting `[`, `]`, `"`) rather than the
/// `MICROMEGAS_ADMINS` JSON-array shape -- duplicated here rather than depending on
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
                 comma-separated, not a JSON array like MICROMEGAS_ADMINS"
            );
        }
        values.push(trimmed.to_string());
    }
    Ok(values)
}

/// Resolves `{prefix}_{suffix}` (falling back to `MICROMEGAS_{suffix}` if unset, or always if
/// `prefix` is empty). Used both by [`IsolationConfig::from_env`]'s `PUBLIC_VIEW_SETS` knob and
/// by its removed-knob check for `UNSTAMPED_AUDIENCE`.
fn resolved_var(prefix: &str, suffix: &str) -> String {
    if prefix.is_empty() {
        format!("MICROMEGAS_{suffix}")
    } else {
        let prefixed = format!("{prefix}_{suffix}");
        if std::env::var(&prefixed).is_ok() {
            prefixed
        } else {
            format!("MICROMEGAS_{suffix}")
        }
    }
}

impl IsolationConfig {
    /// Resolves the surviving knob from the environment. `{prefix}_PUBLIC_VIEW_SETS` unset ⇒ no
    /// public view sets; a malformed entry is `Err`, not silently ignored -- a startup `?` turns
    /// a typo into a fail-fast instead of a silently-inert knob.
    ///
    /// `{prefix}_UNSTAMPED_AUDIENCE` / `MICROMEGAS_UNSTAMPED_AUDIENCE`, if set at all (including
    /// to an empty string), is a startup error rather than being silently ignored: the knob has
    /// never shipped in a release (removed here, in the same #1482 change that introduces it, as
    /// an **Unreleased** CHANGELOG entry), so no released deployment can be relying on it, but a
    /// deployment built from `main` between the two changes might be -- for a fail-closed
    /// posture, silently dropping the knob would be exactly the kind of silent behavior change
    /// this project's env-var conventions exist to avoid. The fix named in the error is
    /// `MICROMEGAS_DEFAULT_INGESTION_AUDIENCE`, the write-side knob that replaces it (#1482 §0).
    pub fn from_env(prefix: &str) -> anyhow::Result<Self> {
        let unstamped_var = resolved_var(prefix, "UNSTAMPED_AUDIENCE");
        if std::env::var(&unstamped_var).is_ok() {
            anyhow::bail!(
                "{unstamped_var} is no longer supported: the audience column is now a physical, \
                 non-nullable column materialized on every global view (#1482), so there is no \
                 more query-time \"unstamped\" fallback to configure. Assign legacy data an \
                 audience with MICROMEGAS_DEFAULT_INGESTION_AUDIENCE on the ingestion side \
                 instead, and remove this variable."
            );
        }
        let public_view_sets_var = resolved_var(prefix, "PUBLIC_VIEW_SETS");
        let public_view_sets = parse_comma_separated_list(&public_view_sets_var)?;
        Ok(Self { public_view_sets })
    }
}
