//! Authorization seam (#1369, AbAC Stage 1): `MintPolicy`, `ReadPolicy`, and the
//! audience-based implementations that resolve them from a caller's `AuthContext` and a
//! comma-separated implicit-groups env var.
//!
//! **No enforcement lands with this module itself.** This module fixes the *shape* of
//! authorization -- every caller of these traits must deny on `Err`, and `ReadPolicy` cannot
//! express "grant everything" at all -- while the resolved `ReadableAudiences`/`ReadScope` is
//! consumed downstream by `OwnershipRewrite` (#1370, AbAC Stage 2; Prong A) and, still pending
//! (#1371, Stage 3), Prong B's UDTF/UDF guards. See `rust/analytics/src/lakehouse/read_scope.rs`
//! and `tasks/1369_policy_seam_plan.md`.

use crate::default_provider::implicit_groups_var;
use crate::types::AuthContext;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use micromegas_tracing::info;
use std::collections::BTreeSet;
use std::fmt::Debug;
use std::sync::Arc;

/// A policy's resolved set of audiences a caller may read.
///
/// Newtype over `Arc<[String]>`, not a bare `Vec<String>`/`Arc<[String]>` -- so a policy's
/// result can never be confused with any other string list on a security path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadableAudiences(Arc<[String]>);

impl ReadableAudiences {
    /// Wraps an already-computed audience set.
    pub fn new(audiences: Arc<[String]>) -> Self {
        Self(audiences)
    }

    /// Unwraps the audience set. This is the one cross-crate conversion point: the
    /// `rust/public` bridge (see `tasks/1369_policy_seam_plan.md` §1) calls this to build
    /// `micromegas_analytics::lakehouse::read_scope::ReadScope::Audiences`.
    pub fn into_inner(self) -> Arc<[String]> {
        self.0
    }
}

/// Resolves the set of audiences an authenticated caller may read.
///
/// `resolve` can **never** return "all" -- `ReadScope::All` is deliberately not a policy output.
/// It is the marker only internal (non-request) callers pass, which is what keeps "who granted
/// themselves `All`?" a greppable question with a small, auditable answer set. A policy that
/// wants to be maximally permissive still has to enumerate the audiences that make it so.
///
/// `async` and fallible because the AbAC plan's recorded *Long-term model* resolves grants from a
/// store -- nested groups, plus group→audience grants -- which cannot live behind a sync,
/// infallible signature. Today's only implementation (`AudienceReadPolicy`) cannot fail; the
/// signature is future-proofing, not present-tense necessity.
///
/// **Every caller must deny on `Err`.** An `Err` here means "the policy could not be evaluated" --
/// never soften it into `ReadableAudiences::new(Arc::from([]))` (that would read as a legitimate,
/// audited fail-closed decision) and never into `ReadScope::All` (a silent fail-open bypass).
#[async_trait]
pub trait ReadPolicy: Send + Sync + Debug {
    /// Resolves `caller`'s readable-audience set, or `Err` if the policy could not be evaluated
    /// (e.g. a backing store outage once a store-backed policy lands).
    async fn resolve(&self, caller: &AuthContext) -> Result<ReadableAudiences>;
}

/// Resolves the audience a newly minted key may be stamped with.
///
/// `requested` is the caller-supplied audience (e.g. `"user:bob@example.com"` or
/// `"group:everyone"`); `None` means "mint a key scoped to myself". Async and fallible for the
/// same forward-looking reason as `ReadPolicy` -- the wiring waits for Stage 6 (`mint_key`), but
/// the trait shape is settled now so Stage 6 adds no further seam.
#[async_trait]
pub trait MintPolicy: Send + Sync + Debug {
    /// Resolves the audience to stamp on a newly minted key, or `Err` if `requested` is outside
    /// what `caller` may mint.
    async fn resolve_audience(
        &self,
        caller: &AuthContext,
        requested: Option<&str>,
    ) -> Result<String>;
}

/// The `{user:<email>} ∪ {group:<g> : g ∈ groups} ∪ {group:<g> : g ∈ implicit_groups}` piece
/// shared by `AudienceReadPolicy::resolve` and `AudienceMintPolicy::resolve_audience`'s non-admin
/// arm (AbAC plan §1-§2).
///
/// Branch-free -- no `auth_type` check anywhere: an OIDC caller carries no `read_audiences`, and
/// an API-key/service-account caller carries no email and no groups claim, so both principal
/// kinds fall out of the same union with no separate `Self*` implementation. With no groups claim
/// and no implicit groups a human caller's set is the singleton `{user:<email>}`.
fn identity_and_group_audiences(
    caller: &AuthContext,
    implicit_groups: &[String],
) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    if let Some(email) = &caller.email {
        set.insert(format!("user:{email}"));
    }
    for g in &caller.groups {
        set.insert(format!("group:{g}"));
    }
    for g in implicit_groups {
        set.insert(format!("group:{g}"));
    }
    set
}

/// `true` if `aud` is a well-formed `user:`/`group:`-prefixed audience (non-empty after the
/// prefix). Used only by `AudienceMintPolicy`'s admin arm -- the non-admin arm never needs it,
/// since a malformed audience can never appear in the mintable set it checks membership against.
fn is_well_formed_audience(aud: &str) -> bool {
    ["user:", "group:"]
        .iter()
        .any(|prefix| aud.len() > prefix.len() && aud.starts_with(prefix))
}

fn default_self_audience(caller: &AuthContext) -> Result<String> {
    caller
        .email
        .as_deref()
        .map(|e| format!("user:{e}"))
        .ok_or_else(|| anyhow!("cannot default a mint audience: caller has no email"))
}

/// Comma-separated implicit-groups parser for `{prefix}_IMPLICIT_GROUPS` /
/// `MICROMEGAS_IMPLICIT_GROUPS`.
///
/// Deliberately **not** the `MICROMEGAS_ADMINS` JSON-array shape
/// (`serde_json::from_str::<Vec<String>>`) -- that variable is the precedent for
/// *config-sourced authorization data* and for the `from_env`-when-unset pattern, not for an
/// encoding. An operator copying that shape here would silently configure one implicit group
/// literally named `["everyone"]`, or two groups `"a"` / `"b"]` (both comma-free after the
/// split, so a bare "reject entries containing a comma" check would never catch them) -- so
/// this rejects any entry containing `[`, `]`, or `"`, or that is empty after trimming, naming
/// the offending entry rather than silently dropping or defaulting it.
fn parse_implicit_groups(var: &str) -> Result<Vec<String>> {
    let raw = match std::env::var(var) {
        Ok(raw) => raw,
        Err(_) => return Ok(vec![]),
    };
    if raw.trim().is_empty() {
        return Ok(vec![]);
    }
    let mut groups = Vec::new();
    for entry in raw.split(',') {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            return Err(anyhow!(
                "{var}: empty group name (entry {entry:?} in {raw:?})"
            ));
        }
        if trimmed.contains(['[', ']', '"']) {
            return Err(anyhow!(
                "{var}: entry {trimmed:?} contains '[', ']', or '\"' -- this variable is \
                 comma-separated, not a JSON array like MICROMEGAS_ADMINS"
            ));
        }
        groups.push(trimmed.to_string());
    }
    Ok(groups)
}

/// The shipped, claims-plus-env `ReadPolicy`. Resolves the readable set:
///
/// ```text
/// caller.read_audiences                    // Stage 4b's service-account read grant
///   ∪ {user:<email>}     if email present  // human OIDC caller
///   ∪ {group:<g>}        for g in groups claim
///   ∪ {group:<g>}        for g in implicit groups
/// ```
///
/// `read_audiences` is folded in here but deliberately **excluded** from
/// `AudienceMintPolicy`'s mintable set -- a service-account key's ability to *see* an audience
/// must not imply it can *stamp new keys* with that audience (mint is integrity, read is
/// confidentiality).
#[derive(Debug, Clone)]
pub struct AudienceReadPolicy {
    implicit_groups: Vec<String>,
}

impl AudienceReadPolicy {
    /// Builds a policy with an explicit implicit-groups list (bypassing env resolution).
    pub fn new(implicit_groups: Vec<String>) -> Self {
        Self { implicit_groups }
    }

    /// Resolves implicit groups from `{prefix}_IMPLICIT_GROUPS` (falling back to
    /// `MICROMEGAS_IMPLICIT_GROUPS`) via [`implicit_groups_var`]. Unset ⇒ empty implicit
    /// groups ⇒ the readable set degenerates to the caller's own singleton, which
    /// `OwnershipRewrite` (#1370, AbAC Stage 2) enforces -- see that stage's CHANGELOG upgrade
    /// note for the `MICROMEGAS_IMPLICIT_GROUPS=everyone` +
    /// `MICROMEGAS_UNSTAMPED_AUDIENCE=group:everyone` pair required to keep legacy,
    /// never-stamped data visible to every caller once enforcement is live. A malformed entry
    /// (see [`parse_implicit_groups`]) is `Err`, not a silently-emptied set, so a startup `?`
    /// turns a typo into a fail-fast instead of a silently-inactive knob.
    pub fn from_env(prefix: &str) -> Result<Self> {
        let var = implicit_groups_var(prefix);
        let implicit_groups = parse_implicit_groups(&var)?;
        if implicit_groups.is_empty() {
            info!("{var}: no implicit groups configured");
        } else {
            info!("{var}: implicit groups = {}", implicit_groups.join(","));
        }
        Ok(Self { implicit_groups })
    }
}

#[async_trait]
impl ReadPolicy for AudienceReadPolicy {
    async fn resolve(&self, caller: &AuthContext) -> Result<ReadableAudiences> {
        let mut set = identity_and_group_audiences(caller, &self.implicit_groups);
        for audience in &caller.read_audiences {
            set.insert(audience.clone());
        }
        let audiences: Arc<[String]> = set.into_iter().collect::<Vec<_>>().into();
        Ok(ReadableAudiences::new(audiences))
    }
}

/// The shipped, claims-plus-env `MintPolicy`. Non-admin callers may mint any audience in the
/// mintable set:
///
/// ```text
/// {user:<email>}       if email present      // human OIDC caller
///   ∪ {group:<g>}       for g in groups claim
///   ∪ {group:<g>}       for g in MICROMEGAS_IMPLICIT_GROUPS
/// ```
///
/// (the same formula as `AudienceReadPolicy`'s, minus `read_audiences` -- see that policy's doc
/// comment for why). `is_admin` callers may mint **any** well-formed audience, including another
/// user's -- `mint_key` is `AdminUser`-gated (`analytics-web-srv/src/ingestion_keys.rs`), so
/// without this arm the only shipped `MintPolicy` could not express the mint flow that exists
/// today; the arm grants no power the route's gate does not already grant. This is deliberately
/// **asymmetric** to the read path, where `is_admin` is never a bypass (AbAC plan §5): mint is an
/// integrity decision (who may stamp a credential), reads are a confidentiality decision (who may
/// see data), and the two axes are allowed to disagree.
#[derive(Debug, Clone)]
pub struct AudienceMintPolicy {
    implicit_groups: Vec<String>,
}

impl AudienceMintPolicy {
    /// Builds a policy with an explicit implicit-groups list.
    pub fn new(implicit_groups: Vec<String>) -> Self {
        Self { implicit_groups }
    }
}

#[async_trait]
impl MintPolicy for AudienceMintPolicy {
    async fn resolve_audience(
        &self,
        caller: &AuthContext,
        requested: Option<&str>,
    ) -> Result<String> {
        if caller.is_admin {
            return match requested {
                None => default_self_audience(caller),
                Some(aud) if is_well_formed_audience(aud) => Ok(aud.to_string()),
                Some(aud) => Err(anyhow!(
                    "malformed audience {aud:?}: must be 'user:<id>' or 'group:<id>'"
                )),
            };
        }
        match requested {
            None => default_self_audience(caller),
            Some(aud) => {
                let mintable = identity_and_group_audiences(caller, &self.implicit_groups);
                if mintable.contains(aud) {
                    Ok(aud.to_string())
                } else {
                    Err(anyhow!(
                        "audience {aud:?} is not in the caller's mintable set"
                    ))
                }
            }
        }
    }
}
