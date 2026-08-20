//! Authorization seam (#1369, AbAC Stage 1; grant map rewrite #1372, Stage 4): `MintPolicy`,
//! `ReadPolicy`, and the audience-based implementations that resolve them from a caller's
//! `AuthContext` and a JSON grant map keyed by audience name.
//!
//! **No enforcement lands with this module itself.** This module fixes the *shape* of
//! authorization -- every caller of these traits must deny on `Err`, and `ReadPolicy` cannot
//! express "grant everything" at all -- while the resolved `ReadableAudiences`/`ReadScope` is
//! consumed downstream by `OwnershipRewrite` (#1370, AbAC Stage 2; Prong A) and, still pending
//! (#1371, Stage 3), Prong B's UDTF/UDF guards. See `rust/analytics/src/lakehouse/read_scope.rs`
//! and `tasks/1372_audience_on_keys_plan.md`.
//!
//! **An audience is an opaque label, not a principal encoding.** `is_valid_audience` and
//! `PUBLIC_AUDIENCE` define what a name is; `AudienceGrants` defines who may read or mint into
//! it. No code here derives an audience from a caller's identity -- see `AudienceGrants` and
//! `tasks/1372_audience_on_keys_plan.md` §1-§2 for the reasoning.

use crate::db_audience_grants::DbAudienceGrantsSource;
use crate::env::resolve_prefixed_var;
use crate::types::AuthContext;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use micromegas_tracing::info;
use serde::Deserialize;
use serde::de::{self, MapAccess, Visitor};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt;
use std::fmt::Debug;
use std::sync::Arc;

/// The reserved audience every authenticated principal may read.
pub const PUBLIC_AUDIENCE: &str = "public";

/// `true` if `aud` is a valid audience name: `[A-Za-z0-9_-]{1,255}`, checked in bytes -- the
/// ASCII-only charset makes byte length and char length agree for every value that can pass, so
/// this doubles as the length check the `ingestion_api_keys.audience` `VARCHAR(255)` column
/// enforces on the SQL side.
///
/// An audience is an opaque label on data, not a principal encoding: this charset makes an email
/// (`@`, `.`) or a `user:`/`group:`-prefixed value (`:`) unrepresentable, on purpose -- see
/// `AudienceGrants` for why there is no derivation from a caller's identity to replace it with.
///
/// Deliberately **not** normalizing: no case folding, no trimming. The value is stored and
/// compared verbatim, which is what makes uniqueness meaningful -- `team-alpha` and `Team-Alpha`
/// are two audiences, and neither is a typo the system silently repairs into the other.
pub fn is_valid_audience(aud: &str) -> bool {
    !aud.is_empty()
        && aud.len() <= 255
        && aud
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Resolves `{prefix}_DEFAULT_KEY_AUDIENCE` (falling back to `MICROMEGAS_DEFAULT_KEY_AUDIENCE`) --
/// resolved **once at startup** (`web_server.rs`) so a typo fails fast rather than surfacing as a
/// per-request 400. `None` when neither is set or the value is empty. Invalid ⇒ `Err`.
///
/// Consumed by `ingestion_keys.rs::resolve_audience`'s `import` fallback (`Some(PUBLIC_AUDIENCE)`)
/// and `mint`'s (`None` -- an unresolved mint is a 400, never a silent `public`). See
/// `tasks/1372_audience_on_keys_plan.md` §5 for why the two routes differ only when this knob is
/// unset.
pub fn default_key_audience_from_env(prefix: &str) -> Result<Option<String>> {
    let var = resolve_prefixed_var(prefix, "DEFAULT_KEY_AUDIENCE");
    let resolved = match std::env::var(&var) {
        Ok(raw) => {
            let raw = raw.trim().to_string();
            if raw.is_empty() {
                None
            } else if is_valid_audience(&raw) {
                Some(raw)
            } else {
                return Err(anyhow!(
                    "{var}: {raw:?} is not a valid audience name -- must match [A-Za-z0-9_-]{{1,255}}"
                ));
            }
        }
        Err(_) => None,
    };
    match &resolved {
        Some(aud) => info!("{var}: default key audience = {aud}"),
        None => info!("{var}: no default key audience configured"),
    }
    Ok(resolved)
}

/// A principal selector on either axis of an [`AudienceGrants`] entry: `*` (any authenticated
/// principal), `user:<email>` (matches `AuthContext.email`), or `group:<g>` (matches any raw
/// value in `AuthContext.groups`). Validated at parse time; `resolve`/`resolve_audience` never
/// see an unrecognized shape.
///
/// `pub` (#1489, AbAC Stage 6a) -- `analytics-web-srv`'s admin grant-write route (a separate
/// crate) needs to run this exact same selector-shape check `parse`/`from_rows` run, not a
/// re-implementation of it.
pub fn valid_selector(selector: &str) -> bool {
    selector == "*"
        || selector
            .strip_prefix("user:")
            .is_some_and(|rest| !rest.is_empty())
        || selector
            .strip_prefix("group:")
            .is_some_and(|rest| !rest.is_empty())
}

fn selector_matches(selector: &str, caller: &AuthContext) -> bool {
    if selector == "*" {
        return true;
    }
    if let Some(email) = selector.strip_prefix("user:") {
        return caller.email.as_deref() == Some(email);
    }
    if let Some(group) = selector.strip_prefix("group:") {
        return caller.groups.iter().any(|g| g == group);
    }
    // Unreachable given `valid_selector` gates every selector at parse time; false rather than
    // panicking keeps this fail-closed if that invariant is ever violated.
    false
}

/// One audience's read/mint selector lists, both already validated by [`AudienceGrants::parse`].
#[derive(Debug, Clone, Default)]
struct GrantEntry {
    read: Vec<String>,
    mint: Vec<String>,
}

/// The Rust side of `audience_grants.axis` (#1489, AbAC Stage 6a) -- which selector list a DB row
/// contributes to, mirroring the JSON grant map's `"read"`/`"mint"` keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantAxis {
    Read,
    Mint,
}

/// The bare-array-or-object shape a single grant-map value may take on the wire, before content
/// validation. An untagged enum rather than a hand-rolled visitor: JSON arrays and objects are
/// syntactically distinct, so serde can dispatch on shape alone, and any value that is neither
/// (a number, a string, `null`, ...) fails here with serde's own type-mismatch error.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawGrantValue {
    /// Read-only shorthand: a bare array is the audience's `read` list, with an implicitly empty
    /// `mint` list -- never derived from `read`.
    Bare(Vec<String>),
    /// The explicit form, needed only when an audience also grants mint authority.
    Object(RawGrantObject),
}

/// The object form of a single grant-map value, broken out of [`RawGrantValue::Object`] so
/// `#[serde(deny_unknown_fields)]` -- which serde cannot apply directly to an enum variant --
/// has somewhere to attach: without it, a misspelled key (`"raed"` for `"read"`) would silently
/// parse into an empty grant instead of failing startup.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGrantObject {
    #[serde(default)]
    read: Vec<String>,
    #[serde(default)]
    mint: Vec<String>,
}

/// Deserializes the top-level `{prefix}_AUDIENCE_GRANTS` object while rejecting a repeated key --
/// `serde_json`'s own `Map` deserialization silently keeps the *last* value for a duplicate key,
/// which would discard an earlier grant list without a word (see `AudienceGrants`'s doc comment).
/// Pulling keys one at a time through `MapAccess`, rather than deserializing straight into a
/// `BTreeMap`, is what makes that duplicate visible before it is lost.
struct RawAudienceGrants(BTreeMap<String, RawGrantValue>);

impl<'de> Deserialize<'de> for RawAudienceGrants {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct GrantsVisitor;

        impl<'de> Visitor<'de> for GrantsVisitor {
            type Value = RawAudienceGrants;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "a JSON object keyed by audience name")
            }

            fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut result = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    let value: RawGrantValue = map.next_value()?;
                    if result.contains_key(&key) {
                        return Err(de::Error::custom(format!(
                            "duplicate audience key {key:?} in grant map"
                        )));
                    }
                    result.insert(key, value);
                }
                Ok(RawAudienceGrants(result))
            }
        }

        deserializer.deserialize_map(GrantsVisitor)
    }
}

/// The parsed, validated `{prefix}_AUDIENCE_GRANTS` map: "who can access this audience", keyed by
/// audience name, one relation per axis (read/mint -- AbAC plan's `group_read_grants` /
/// `group_mint_grants`, kept as one env map only because there is no store yet to split them
/// across).
///
/// A bare-array value is read-only shorthand: an omitted `"mint"` list is therefore always empty,
/// never defaulted from `"read"` -- a read grant confers no mint authority, by construction.
///
/// `public` is not stored here: it is the sole built-in read grant, applied by
/// `AudienceReadPolicy::resolve` directly rather than needing a `{"public": ["*"]}` entry (though
/// writing one changes nothing).
#[derive(Debug, Clone, Default)]
pub struct AudienceGrants {
    entries: BTreeMap<String, GrantEntry>,
}

impl AudienceGrants {
    /// No grants at all -- every audience but `public` is unreadable and unmintable by anyone
    /// non-admin. Used as the disabled-auth / no-explicit-policy default.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Parses a `{prefix}_AUDIENCE_GRANTS` JSON string directly. Split out from
    /// [`Self::from_env`] so tests can exercise parsing without mutating the environment.
    ///
    /// Validates both axes of the map, not just its JSON shape -- same reason every other knob in
    /// this crate fails startup on a typo rather than shipping inert: every key must satisfy
    /// [`is_valid_audience`] (`{"group:everyone": [...]}`, the value operators are migrating
    /// *from*, can never match a stamped audience once the charset excludes `:`), and every
    /// selector on either axis must be `*`, `user:<non-empty>`, or `group:<non-empty>` (an
    /// unprefixed or mis-prefixed selector matches no identity axis and would otherwise be a
    /// silently-inert entry).
    pub fn parse(raw: &str) -> Result<Self> {
        let raw: RawAudienceGrants =
            serde_json::from_str(raw).map_err(|e| anyhow!("invalid audience grant map: {e}"))?;
        let mut entries = BTreeMap::new();
        for (audience, value) in raw.0 {
            if !is_valid_audience(&audience) {
                return Err(anyhow!(
                    "invalid audience grant map: {audience:?} is not a valid audience name -- \
                     must match [A-Za-z0-9_-]{{1,255}}"
                ));
            }
            let (read, mint) = match value {
                RawGrantValue::Bare(read) => (read, vec![]),
                RawGrantValue::Object(RawGrantObject { read, mint }) => (read, mint),
            };
            for selector in read.iter().chain(mint.iter()) {
                if !valid_selector(selector) {
                    return Err(anyhow!(
                        "invalid audience grant map: selector {selector:?} for audience \
                         {audience:?} must be '*', 'user:<id>', or 'group:<id>'"
                    ));
                }
            }
            entries.insert(audience, GrantEntry { read, mint });
        }
        Ok(Self { entries })
    }

    /// Resolves [`audience_grants_var`] and parses it. Unset ⇒ [`Self::empty`].
    pub fn from_env(prefix: &str) -> Result<Self> {
        let var = resolve_prefixed_var(prefix, "AUDIENCE_GRANTS");
        let grants = match std::env::var(&var) {
            Ok(raw) if raw.trim().is_empty() => Self::empty(),
            Ok(raw) => Self::parse(&raw).map_err(|e| anyhow!("{var}: {e}"))?,
            Err(_) => Self::empty(),
        };
        if grants.entries.is_empty() {
            info!("{var}: no audience grants configured");
        } else {
            let names = grants.entries.keys().cloned().collect::<Vec<_>>().join(",");
            info!("{var}: {} audience grants ({names})", grants.entries.len());
        }
        Ok(grants)
    }

    /// Builds an `AudienceGrants` from `(audience, axis, selector)` triples -- the DB-store
    /// analogue of [`Self::parse`] (#1489, AbAC Stage 6a). Runs the *same* [`is_valid_audience`]/
    /// [`valid_selector`] checks `parse` runs, so a row that slipped past `audience_grants`'s own
    /// `CHECK` constraints (e.g. via a direct `psql` session) still can't reach a policy decision:
    /// this is the one place both the JSON path and the DB path fail closed on a malformed row.
    pub fn from_rows(rows: impl IntoIterator<Item = (String, GrantAxis, String)>) -> Result<Self> {
        let mut entries: BTreeMap<String, GrantEntry> = BTreeMap::new();
        for (audience, axis, selector) in rows {
            if !is_valid_audience(&audience) {
                return Err(anyhow!(
                    "invalid audience grant row: {audience:?} is not a valid audience name -- \
                     must match [A-Za-z0-9_-]{{1,255}}"
                ));
            }
            if !valid_selector(&selector) {
                return Err(anyhow!(
                    "invalid audience grant row: selector {selector:?} for audience \
                     {audience:?} must be '*', 'user:<id>', or 'group:<id>'"
                ));
            }
            let entry = entries.entry(audience).or_default();
            match axis {
                GrantAxis::Read => entry.read.push(selector),
                GrantAxis::Mint => entry.mint.push(selector),
            }
        }
        Ok(Self { entries })
    }

    /// Unions each audience's `read`/`mint` selector lists across `self` and `other` (#1489, AbAC
    /// Stage 6a): a selector present in either map grants exactly the same access as being
    /// present in both. No dedup -- `selector_matches` is called with `.any()`, so a selector
    /// present in both sources costs one redundant comparison, never a wrong answer.
    ///
    /// A public utility for combining two `AudienceGrants` maps, exercised by this crate's own
    /// integration tests (`rust/auth/tests/policy_tests.rs`); `resolve`/`resolve_audience` do
    /// **not** call it -- they check the env map and the DB store snapshot as two separate
    /// sources instead, specifically to avoid a per-request deep clone (see the comment in
    /// [`AudienceReadPolicy::resolve`]).
    pub fn merge(&self, other: &Self) -> Self {
        let mut entries = self.entries.clone();
        for (audience, other_entry) in &other.entries {
            let entry = entries.entry(audience.clone()).or_default();
            entry.read.extend(other_entry.read.iter().cloned());
            entry.mint.extend(other_entry.mint.iter().cloned());
        }
        Self { entries }
    }

    fn readers(&self) -> impl Iterator<Item = (&String, &[String])> {
        self.entries.iter().map(|(a, e)| (a, e.read.as_slice()))
    }

    fn mint_selectors(&self, audience: &str) -> &[String] {
        self.entries
            .get(audience)
            .map(|e| e.mint.as_slice())
            .unwrap_or(&[])
    }
}

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
/// infallible signature. This is no longer just future-proofing: `AudienceReadPolicy` is fallible
/// today whenever a [`DbAudienceGrantsSource`] is attached via `with_store` -- a cold-start store
/// outage with no prior successful load returns `Err`, which `caller_context`
/// (`flight_sql_service_impl.rs`) maps to `Status::unavailable`. Without a store attached it still
/// cannot fail.
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
/// `requested` is the caller-supplied audience (e.g. `"team-alpha"`); `None` means "no audience
/// requested and none can be defaulted" -- under the opaque-label model there is no "myself"
/// audience for a caller's identity to default to, so every implementation must return `Err` for
/// `requested: None`, admin or not. Async and fallible for the same forward-looking reason as
/// `ReadPolicy` -- wired by `mint_key` (`analytics-web-srv/src/ingestion_keys.rs`, AbAC Stage 6,
/// #1374), its first production caller, gated by `MintGate`/`AuthenticatedUser` rather than
/// `AdminUser`.
#[async_trait]
pub trait MintPolicy: Send + Sync + Debug {
    /// Resolves the audience to stamp on a newly minted key, or `Err` if `requested` is outside
    /// what `caller` may mint (including `requested: None` -- see the trait doc comment).
    async fn resolve_audience(
        &self,
        caller: &AuthContext,
        requested: Option<&str>,
    ) -> Result<String>;
}

/// The shipped `ReadPolicy`: a lookup over an [`AudienceGrants`] map -- and, when a
/// [`DbAudienceGrantsSource`] is attached via `with_store`, a second lookup over that store's
/// snapshot -- with no identity derivation anywhere. Resolves the readable set:
///
/// ```text
/// { PUBLIC_AUDIENCE }
///   ∪ { a : "*"            ∈ grants[a].read }
///   ∪ { a : "user:<email>" ∈ grants[a].read }                  if email present
///   ∪ { a : "group:<g>"    ∈ grants[a].read for some g ∈ caller.groups }
///   ∪ { a : selector       ∈ store.readers(a) matches caller } if a store is attached
///   ∪ caller.read_audiences                                    // Stage 4b per-key direct grant
/// ```
///
/// There is **no self-audience rule** -- a caller is granted no audience merely for being named
/// like one. See `tasks/1372_audience_on_keys_plan.md` §2 for why: the charset makes an email
/// unrepresentable as an audience name, and keying on `subject` instead would let an admin mint
/// themselves read access by naming a key after an audience.
///
/// `read_audiences` is folded in here but deliberately **excluded** from
/// `AudienceMintPolicy`'s mintable set -- a service-account key's ability to *see* an audience
/// must not imply it can *stamp new keys* with that audience (mint is integrity, read is
/// confidentiality).
#[derive(Debug, Clone, Default)]
pub struct AudienceReadPolicy {
    grants: AudienceGrants,
    /// The DB-backed grant store (#1489, AbAC Stage 6a), when this process has one configured --
    /// checked alongside `grants` on every `resolve` call, as a separate source rather than
    /// merged into it (see `resolve`). `None` for every disabled-auth/test caller that has no DB
    /// pool to back one.
    store: Option<Arc<DbAudienceGrantsSource>>,
}

impl AudienceReadPolicy {
    /// Builds a policy with an explicit grant map (bypassing env resolution).
    pub fn new(grants: AudienceGrants) -> Self {
        Self {
            grants,
            store: None,
        }
    }

    /// Resolves the grant map from `{prefix}_AUDIENCE_GRANTS` (falling back to
    /// `MICROMEGAS_AUDIENCE_GRANTS`) via [`AudienceGrants::from_env`]. Unset ⇒ an empty grant map
    /// ⇒ the readable set degenerates to `{public}` plus `read_audiences`, which
    /// `OwnershipRewrite` (#1370, AbAC Stage 2) enforces. A malformed grant map is `Err`, not a
    /// silently-emptied one, so a startup `?` turns a typo into a fail-fast instead of a
    /// silently-inactive knob.
    pub fn from_env(prefix: &str) -> Result<Self> {
        let grants = AudienceGrants::from_env(prefix)?;
        Ok(Self {
            grants,
            store: None,
        })
    }

    /// Attaches (or clears, with `None`) the DB-backed grant store (#1489, AbAC Stage 6a). A
    /// builder method, not a constructor argument, so `new`/`from_env` keep working unchanged for
    /// every caller with no DB pool to back one (disabled-auth, tests).
    pub fn with_store(mut self, store: Option<Arc<DbAudienceGrantsSource>>) -> Self {
        self.store = store;
        self
    }
}

#[async_trait]
impl ReadPolicy for AudienceReadPolicy {
    async fn resolve(&self, caller: &AuthContext) -> Result<ReadableAudiences> {
        // `Err` only on a cold-start outage (no snapshot has ever loaded successfully) --
        // propagated as-is, so `resolve` denies exactly as documented on its own trait.
        let store_grants = match &self.store {
            Some(store) => Some(store.current().await?),
            None => None,
        };
        let mut set = BTreeSet::new();
        set.insert(PUBLIC_AUDIENCE.to_string());
        // The env map and the store snapshot are checked as two separate sources -- a selector
        // present in either grants access -- rather than merged into one map, so neither side is
        // deep-cloned on every request (#1489, AbAC Stage 6a).
        for (audience, read) in self.grants.readers() {
            if read.iter().any(|s| selector_matches(s, caller)) {
                set.insert(audience.clone());
            }
        }
        if let Some(grants) = &store_grants {
            for (audience, read) in grants.readers() {
                if read.iter().any(|s| selector_matches(s, caller)) {
                    set.insert(audience.clone());
                }
            }
        }
        for audience in &caller.read_audiences {
            set.insert(audience.clone());
        }
        let audiences: Arc<[String]> = set.into_iter().collect::<Vec<_>>().into();
        Ok(ReadableAudiences::new(audiences))
    }
}

/// The shipped `MintPolicy`. Non-admin callers may mint only an audience in their **mint** set --
/// `grants[a].mint`, never `grants[a].read` (a read grant confers no mint authority, unchanged
/// from `AudienceReadPolicy`'s split); being able to *read* `public`, which every authenticated
/// principal is, does not imply being able to *mint into* it, unless some grant names `public` in
/// a `"mint"` list.
///
/// `is_admin` callers may mint **any** valid audience, `public` included -- `mint_key`
/// (`analytics-web-srv/src/ingestion_keys.rs`) delegates authorization to this policy for every
/// caller, admin included, rather than gating admin callers separately of its own accord (its own
/// gate, `MintGate`, only enforces the self-service knob against non-admins); without this arm the
/// only shipped `MintPolicy` could not express the admin mint flow that route depends on; the arm
/// grants no power the route's gate does not already grant. This is deliberately **asymmetric** to
/// the read path, where `is_admin` is never a bypass (AbAC plan §5): mint is an integrity decision
/// (who may stamp a credential), reads are a confidentiality decision (who may see data), and the
/// two axes are allowed to disagree.
#[derive(Debug, Clone, Default)]
pub struct AudienceMintPolicy {
    grants: AudienceGrants,
    /// The DB-backed grant store (#1489, AbAC Stage 6a). Built for symmetry with
    /// [`AudienceReadPolicy::with_store`] and unit-tested, but this stage wires nothing to it: no
    /// production code constructs a `dyn MintPolicy` today -- see [`with_store`](Self::with_store).
    store: Option<Arc<DbAudienceGrantsSource>>,
}

impl AudienceMintPolicy {
    /// Builds a policy with an explicit grant map.
    pub fn new(grants: AudienceGrants) -> Self {
        Self {
            grants,
            store: None,
        }
    }

    /// Attaches (or clears, with `None`) the DB-backed grant store (#1489, AbAC Stage 6a). Built
    /// alongside [`AudienceReadPolicy::with_store`] for symmetry -- unlike the read side, no
    /// production call site attaches a store through this method: `mint_key` (AbAC Stage 6,
    /// #1374) constructs its `AudienceMintPolicy` via `new`, so mint grants stay resolved by a
    /// fresh, uncached point query against `audience_grants`, never a `DbAudienceGrantsSource`
    /// snapshot, in this stage.
    pub fn with_store(mut self, store: Option<Arc<DbAudienceGrantsSource>>) -> Self {
        self.store = store;
        self
    }
}

#[async_trait]
impl MintPolicy for AudienceMintPolicy {
    async fn resolve_audience(
        &self,
        caller: &AuthContext,
        requested: Option<&str>,
    ) -> Result<String> {
        let Some(aud) = requested else {
            return Err(anyhow!("no audience requested and none can be defaulted"));
        };
        if caller.is_admin {
            return if is_valid_audience(aud) {
                Ok(aud.to_string())
            } else {
                Err(anyhow!(
                    "malformed audience {aud:?}: must match [A-Za-z0-9_-]{{1,255}}"
                ))
            };
        }
        // The env map and the store snapshot are checked as two separate sources -- a selector
        // present in either grants access -- rather than merged into one map, so neither side is
        // deep-cloned on every request (#1489, AbAC Stage 6a).
        let store_grants = match &self.store {
            Some(store) => Some(store.current().await?),
            None => None,
        };
        let granted = self
            .grants
            .mint_selectors(aud)
            .iter()
            .any(|s| selector_matches(s, caller))
            || store_grants.as_ref().is_some_and(|grants| {
                grants
                    .mint_selectors(aud)
                    .iter()
                    .any(|s| selector_matches(s, caller))
            });
        if granted {
            Ok(aud.to_string())
        } else {
            Err(anyhow!(
                "audience {aud:?} is not in the caller's mintable set"
            ))
        }
    }
}
