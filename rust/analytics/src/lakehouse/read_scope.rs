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
//! structurally cannot reach, plus the [`IsolationConfig::user_maintenance_functions`]
//! registration knob.

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
    /// The audience scope to plan queries under. Consumed by
    /// [`super::ownership_rewrite::OwnershipRewrite`] (#1370, AbAC Stage 2) inside
    /// `query.rs::make_session_context`.
    pub read_scope: ReadScope,
    /// Whether the caller may use the five mutating lakehouse UDTFs/UDFs (unchanged from
    /// today's `is_admin: bool` parameter).
    pub is_admin: bool,
    /// Per-service data-isolation deployment config (`MICROMEGAS_UNSTAMPED_AUDIENCE`,
    /// `MICROMEGAS_PUBLIC_VIEW_SETS`, `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS`) -- resolved once
    /// at server startup, not per request, but bundled here rather than as a new
    /// `make_session_context` parameter (#1370 Design §8): per-request resolved values ride the
    /// context, per-service objects live on the service, and this rides along with `read_scope`
    /// at every real call site anyway. Named `isolation_config`, not `ownership_config`: it
    /// carries knobs consumed by both Prong A (`OwnershipRewrite`, #1370) and Prong B
    /// (`AudienceGuard`'s registration gate, #1371), not just the ownership rewrite.
    pub isolation_config: Arc<IsolationConfig>,
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
        }
    }

    /// For background/materialization callers performing maintenance work (`is_admin: true`,
    /// `ReadScope::All`) -- never a user session.
    pub fn maintenance() -> Self {
        Self {
            read_scope: ReadScope::All,
            is_admin: true,
            isolation_config: Arc::new(IsolationConfig::default()),
        }
    }
}

/// Deployment config for the data-isolation seam: [`super::ownership_rewrite::OwnershipRewrite`]
/// (#1370, AbAC Stage 2, Prong A) and the mutating-function registration gate
/// (#1371, AbAC Stage 3, Prong B). Per-service, resolved once at server startup from environment
/// variables -- see [`IsolationConfig::from_env`]. Named for what it configures (data isolation),
/// not for either prong individually -- it predates Prong B's own name and was renamed from
/// `OwnershipRewriteConfig` when this third knob landed, per `CLAUDE.md`'s "Rust API surface may
/// change freely" stance: a clean name beats a compatible one here, and this rename touches only
/// Rust construction sites, none of them SQL-layer surface.
#[derive(Debug, Clone, Default)]
pub struct IsolationConfig {
    /// The audience to fall back to (via `coalesce`) for a process whose resolved audience is
    /// `NULL` (never stamped with `micromegas.audience`) -- e.g. `"public"`. `None`
    /// (the default) means unstamped processes stay invisible to every `ReadScope::Audiences`
    /// caller. Parsed from `{prefix}_UNSTAMPED_AUDIENCE`, falling back to
    /// `MICROMEGAS_UNSTAMPED_AUDIENCE`.
    pub unstamped_audience: Option<String>,
    /// View-set names `OwnershipRewrite` skips entirely -- no predicate injected at all,
    /// regardless of scope. Off (empty) by default, matching the AbAC plan's "off by default,
    /// fail-closed" framing for this operator-responsibility allowlist. Parsed from
    /// `{prefix}_PUBLIC_VIEW_SETS`, falling back to `MICROMEGAS_PUBLIC_VIEW_SETS`.
    pub public_view_sets: Vec<String>,
    /// Registers the five mutating lakehouse UDTFs/UDFs (`retire_partitions`,
    /// `materialize_partitions`, `regenerate_partitions`, `retire_partition_by_file`,
    /// `retire_partition_by_metadata`) for *every* caller, not just an admin (`query.rs`'s gate
    /// becomes `caller.is_admin || isolation_config.user_maintenance_functions`). Off (`false`)
    /// by default. Meant for an API-key-only deployment: an API key can never be admin
    /// (`api_key.rs`), so without this knob such a deployment has no admin principal at all and
    /// no access to these functions whatsoever. **Deployment-wide, not per-audience** -- none of
    /// the five functions filters by audience, so enabling this grants *any* authenticated caller
    /// destructive access to *every* audience's partitions, not just their own; safe only when no
    /// admin principal exists, unsafe the moment the same deployment also has personal or
    /// per-team audiences (tighten to per-audience checks if that becomes real; out of scope
    /// here). Parsed from `{prefix}_USER_MAINTENANCE_FUNCTIONS`, falling back to
    /// `MICROMEGAS_USER_MAINTENANCE_FUNCTIONS`.
    pub user_maintenance_functions: bool,
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

/// Parses a `{var}` env var as a strict boolean: `"true"`/`"false"` (case-insensitive), `Err` on
/// anything else -- matching the fail-fast posture of the other two knobs rather than silently
/// defaulting a typo to `false`. Unset *or* empty/whitespace-only ⇒ `false` (the knob's
/// off-by-default posture), the same "empty means unset" treatment `unstamped_audience` and
/// `public_view_sets` give their own vars -- routine in k8s manifests, docker-compose
/// `environment:` lists, and systemd `EnvironmentFile`s, where declaring a var with an empty
/// value is common.
fn parse_bool_var(var: &str) -> anyhow::Result<bool> {
    match std::env::var(var) {
        Ok(raw) if raw.trim().is_empty() => Ok(false),
        Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => anyhow::bail!(
                "{var}: {raw:?} is not a valid boolean -- must be \"true\" or \"false\" \
                 (case-insensitive)"
            ),
        },
        Err(_) => Ok(false),
    }
}

/// Resolves `{prefix}_{suffix}` (falling back to `MICROMEGAS_{suffix}` if unset, or always if
/// `prefix` is empty). Shared by all three `IsolationConfig::from_env` knobs
/// (`"UNSTAMPED_AUDIENCE"`, `"PUBLIC_VIEW_SETS"`, `"USER_MAINTENANCE_FUNCTIONS"`).
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
    /// Resolves all three knobs from the environment. Unset ⇒ `IsolationConfig::default()`
    /// (unstamped processes stay invisible, no public view sets, mutating functions stay
    /// admin-only). A malformed `{prefix}_UNSTAMPED_AUDIENCE` (outside `[A-Za-z0-9_-]{1,255}`),
    /// a malformed `{prefix}_PUBLIC_VIEW_SETS` entry, or a `{prefix}_USER_MAINTENANCE_FUNCTIONS`
    /// value other than `true`/`false` is `Err`, not silently ignored -- a startup `?` turns a
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
            Err(_) => None,
        };
        let public_view_sets_var = resolved_var(prefix, "PUBLIC_VIEW_SETS");
        let public_view_sets = parse_comma_separated_list(&public_view_sets_var)?;
        let user_maintenance_functions_var = resolved_var(prefix, "USER_MAINTENANCE_FUNCTIONS");
        let user_maintenance_functions = parse_bool_var(&user_maintenance_functions_var)?;
        Ok(Self {
            unstamped_audience,
            public_view_sets,
            user_maintenance_functions,
        })
    }
}
