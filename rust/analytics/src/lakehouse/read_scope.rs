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
    /// Per-service data-isolation deployment config (`MICROMEGAS_UNSTAMPED_AUDIENCE`,
    /// `MICROMEGAS_PUBLIC_VIEW_SETS`) -- resolved once at server startup, not per request, but
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

/// The audience [`IsolationConfig::from_env`] falls back to for `unstamped_audience` when
/// `{prefix}_UNSTAMPED_AUDIENCE`/`MICROMEGAS_UNSTAMPED_AUDIENCE` is unset -- an operator who wants
/// the old fail-closed behavior (unstamped processes invisible to every `ReadScope::Audiences`
/// caller) sets the var to an empty string explicitly rather than leaving it unset.
pub const DEFAULT_UNSTAMPED_AUDIENCE: &str = "public";

/// Deployment config for the data-isolation seam: [`super::ownership_rewrite::OwnershipRewrite`]
/// (#1370, AbAC Stage 2, Prong A). Per-service, resolved once at server startup from environment
/// variables -- see [`IsolationConfig::from_env`].
///
/// `Default` matches [`Self::from_env`]'s own default: `unstamped_audience:
/// Some(`[`DEFAULT_UNSTAMPED_AUDIENCE`]`)`. Internal/maintenance callers plan under
/// `ReadScope::All`, which `OwnershipRewrite`/`AudienceGuard` treat as a true no-op regardless of
/// this value, so `Default` deliberately doesn't carve out a different, `None`-based value just
/// for them -- one struct, one meaning of "unconfigured."
#[derive(Debug, Clone)]
pub struct IsolationConfig {
    /// The audience to fall back to (via `coalesce`) for a process whose resolved audience is
    /// `NULL` (never stamped with `micromegas.audience`) -- e.g. `"public"`. `None` means
    /// unstamped processes stay invisible to every `ReadScope::Audiences` caller; both `Default`
    /// and [`IsolationConfig::from_env`] resolve to `Some(`[`DEFAULT_UNSTAMPED_AUDIENCE`]`)`
    /// instead -- an operator (or a direct struct literal) opts into `None` explicitly. Parsed
    /// from `{prefix}_UNSTAMPED_AUDIENCE`, falling back to `MICROMEGAS_UNSTAMPED_AUDIENCE`.
    pub unstamped_audience: Option<String>,
    /// View-set names `OwnershipRewrite` skips entirely -- no predicate injected at all,
    /// regardless of scope. Off (empty) by default, matching the AbAC plan's "off by default,
    /// fail-closed" framing for this operator-responsibility allowlist. Parsed from
    /// `{prefix}_PUBLIC_VIEW_SETS`, falling back to `MICROMEGAS_PUBLIC_VIEW_SETS`.
    pub public_view_sets: Vec<String>,
}

impl Default for IsolationConfig {
    fn default() -> Self {
        Self {
            unstamped_audience: Some(DEFAULT_UNSTAMPED_AUDIENCE.to_string()),
            public_view_sets: Vec::new(),
        }
    }
}

/// `true` if `aud` is a valid audience name: `[A-Za-z0-9_-]{1,255}`, checked in bytes -- the same
/// two rules as `micromegas_auth::policy::is_valid_audience`, duplicated here rather than
/// depending on `micromegas-auth` for it (see this module's doc comment). An audience outside
/// this charset would silently never match any `ReadScope::Audiences` element (every one of those
/// passed this same check on its way in), so `from_env` rejects it at parse time rather than
/// shipping a configured-but-inert knob.
fn is_well_formed_audience(aud: &str) -> bool {
    !aud.is_empty()
        && aud.len() <= 255
        && aud
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
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
/// `prefix` is empty). Shared by both `IsolationConfig::from_env` knobs
/// (`"UNSTAMPED_AUDIENCE"`, `"PUBLIC_VIEW_SETS"`).
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
    /// Resolves both knobs from the environment. `{prefix}_PUBLIC_VIEW_SETS` unset ⇒ no public
    /// view sets. `{prefix}_UNSTAMPED_AUDIENCE` unset ⇒ [`DEFAULT_UNSTAMPED_AUDIENCE`] --
    /// unstamped processes are visible to that audience unless an operator opts back into the
    /// fail-closed behavior by setting the var to an empty string explicitly (the
    /// all-whitespace/empty branch below, kept distinct from "unset" for exactly this reason). A
    /// malformed `{prefix}_UNSTAMPED_AUDIENCE` (outside `[A-Za-z0-9_-]{1,255}`) or a malformed
    /// `{prefix}_PUBLIC_VIEW_SETS` entry is `Err`, not silently ignored -- a startup `?` turns a
    /// typo into a fail-fast instead of a silently-inert knob.
    pub fn from_env(prefix: &str) -> anyhow::Result<Self> {
        let unstamped_var = resolved_var(prefix, "UNSTAMPED_AUDIENCE");
        let unstamped_audience = match std::env::var(&unstamped_var) {
            Ok(raw) if raw.trim().is_empty() => None,
            Ok(raw) => {
                let raw = raw.trim().to_string();
                if !is_well_formed_audience(&raw) {
                    anyhow::bail!(
                        "{unstamped_var}: {raw:?} is not a valid audience -- must match \
                         [A-Za-z0-9_-]{{1,255}}"
                    );
                }
                Some(raw)
            }
            Err(_) => Some(DEFAULT_UNSTAMPED_AUDIENCE.to_string()),
        };
        let public_view_sets_var = resolved_var(prefix, "PUBLIC_VIEW_SETS");
        let public_view_sets = parse_comma_separated_list(&public_view_sets_var)?;
        Ok(Self {
            unstamped_audience,
            public_view_sets,
        })
    }
}
