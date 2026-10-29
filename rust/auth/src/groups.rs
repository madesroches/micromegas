//! Local group membership: the `groups`/`group_members` tables (migration v10,
//! `rust/ingestion/src/sql_migration.rs`), a whole-table snapshot cache ([`crate::groups::DbGroupsSource`])
//! mirroring [`crate::db_audience_grants::DbAudienceGrantsSource`], and [`crate::groups::GroupGraph`], the
//! in-memory closure resolver over a loaded snapshot.
//!
//! A `group_members.member` row is a selector in exactly the vocabulary
//! `audience_grants.selector` uses (`*`, `user:<email>`, `group:<name>`, `key:<key_id>`), so nesting is the
//! `group:` arm of the same predicate rather than a special case -- see [`crate::groups::GroupGraph::closure`].

use crate::db_api_key::resolve_u64;
use crate::db_snapshot::{SnapshotLoader, SnapshotSource};
use crate::policy::{key_selector, valid_selector};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use sqlx::{PgPool, Row};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use uuid::Uuid;

/// The reserved group whose membership is admin-ness -- `AuthContext::is_admin` is exactly
/// "does this caller's closure contain this name".
pub const ADMINS_GROUP: &str = "admins";

/// `true` if `name` is a valid group name: `[A-Za-z0-9_-]{1,255}` -- the same charset
/// `is_valid_audience` uses, checked in bytes, so a group name is URL-safe and a distinct kind of
/// thing from an email.
pub fn is_valid_group_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A loaded, in-memory snapshot of the `groups`/`group_members` tables, and the closure resolver
/// over it. Sync and infallible once built: every method here is a walk over an already-loaded
/// snapshot, never a query.
#[derive(Debug, Clone, Default)]
pub struct GroupGraph {
    /// Every group name, whether or not it has any members.
    groups: BTreeSet<String>,
    /// member selector (`*`, `user:<email>`, `group:<name>`, or `key:<key_id>`) -> the groups it is a *direct*
    /// member of.
    members_of: BTreeMap<String, Vec<String>>,
}

impl GroupGraph {
    /// Builds a graph from `(name)` rows out of `groups` and `(group_name, member)` rows out of
    /// `group_members`. Runs the name charset and [`valid_selector`] checks the way
    /// `AudienceGrants::from_rows` does -- the tables carry no selector-shape CHECK, so this is
    /// where a malformed row (e.g. from a direct `psql` session) fails the snapshot load
    /// rather than reaching a decision.
    pub fn from_rows(
        groups: impl IntoIterator<Item = String>,
        members: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self> {
        let mut group_set = BTreeSet::new();
        for name in groups {
            if !is_valid_group_name(&name) {
                return Err(anyhow!(
                    "invalid group row: {name:?} is not a valid group name -- must match \
                     [A-Za-z0-9_-]{{1,255}}"
                ));
            }
            group_set.insert(name);
        }
        let mut members_of: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (group_name, member) in members {
            if !is_valid_group_name(&group_name) {
                return Err(anyhow!(
                    "invalid group_members row: {group_name:?} is not a valid group name -- \
                     must match [A-Za-z0-9_-]{{1,255}}"
                ));
            }
            if !valid_selector(&member) {
                return Err(anyhow!(
                    "invalid group_members row: selector {member:?} for group {group_name:?} \
                     must be '*', 'user:<id>', 'group:<id>', or 'key:<key_id>'"
                ));
            }
            members_of.entry(member).or_default().push(group_name);
        }
        Ok(Self {
            groups: group_set,
            members_of,
        })
    }

    /// Every known group name, sorted.
    pub fn group_names(&self) -> impl Iterator<Item = &String> {
        self.groups.iter()
    }

    /// Whether `name` is a known group.
    pub fn contains_group(&self, name: &str) -> bool {
        self.groups.contains(name)
    }

    /// Breadth-first walk upward, starting from `initial` and repeatedly following
    /// `members_of["group:g"]` for each newly reached group `g`. The visited set is what
    /// tolerates a cycle at read time. A `barrier` group is never inserted nor traversed, so
    /// nothing reachable only through it is either.
    fn walk_upward(&self, initial: BTreeSet<String>, barrier: Option<&str>) -> BTreeSet<String> {
        let mut initial = initial;
        if let Some(b) = barrier {
            initial.remove(b);
        }
        let mut visited = initial.clone();
        let mut queue: VecDeque<String> = initial.into_iter().collect();
        while let Some(g) = queue.pop_front() {
            if let Some(next) = self.members_of.get(&format!("group:{g}")) {
                for ng in next {
                    if barrier == Some(ng.as_str()) {
                        continue;
                    }
                    if visited.insert(ng.clone()) {
                        queue.push_back(ng.clone());
                    }
                }
            }
        }
        visited
    }

    /// The caller's resolved, transitive group membership: a breadth-first walk **upward** from
    /// the caller, seeded at the groups listed under the selectors `*` and `user:<email>` (when
    /// `email` is `Some`), unioned with the walk from `key:<key_id>` (when `key_id` is `Some`).
    /// The key walk never reaches [`ADMINS_GROUP`] -- a key is never made admin by membership,
    /// whether listed directly, nested, or via a group above `admins`. The `*` walk is not
    /// barred, so a bootstrap `('admins', '*')` row still makes every caller admin. Returned
    /// sorted, deduplicated. Sync and infallible: the graph is an already-loaded snapshot.
    pub fn closure(&self, email: Option<&str>, key_id: Option<Uuid>) -> Vec<String> {
        let mut initial = BTreeSet::new();
        if let Some(gs) = self.members_of.get("*") {
            initial.extend(gs.iter().cloned());
        }
        if let Some(email) = email
            && let Some(gs) = self.members_of.get(&format!("user:{email}"))
        {
            initial.extend(gs.iter().cloned());
        }
        let mut result = self.walk_upward(initial, None);
        if let Some(key_id) = key_id
            && let Some(gs) = self.members_of.get(&key_selector(key_id))
        {
            result.extend(self.walk_upward(gs.iter().cloned().collect(), Some(ADMINS_GROUP)));
        }
        result.into_iter().collect()
    }

    /// `true` when the wildcard selector `*` reaches [`ADMINS_GROUP`] -- directly (`('admins',
    /// '*')`) or through nesting. Reuses the same upward walk as [`Self::closure`], seeded at `*`
    /// alone (no caller email).
    pub fn has_wildcard_admin(&self) -> bool {
        self.closure(None, None).iter().any(|g| g == ADMINS_GROUP)
    }

    /// `true` when `target` is reachable from *some* principal -- the wildcard `*` selector or
    /// any `user:<email>` selector present in the snapshot -- directly or through nesting.
    /// Generalizes [`Self::has_wildcard_admin`] (which seeds the walk from `*` alone) to seed
    /// from every base selector on record, so a caller can check "would `target` still be
    /// reachable by anyone" rather than "by `*` specifically" -- e.g. before removing a
    /// membership that would strand `ADMINS_GROUP` behind a user-only or nested-only path.
    pub fn any_principal_reaches(&self, target: &str) -> bool {
        let mut initial = BTreeSet::new();
        for (selector, groups) in &self.members_of {
            if selector == "*" || selector.starts_with("user:") {
                initial.extend(groups.iter().cloned());
            }
        }
        self.walk_upward(initial, None).iter().any(|g| g == target)
    }

    /// Whether adding `group:nested` as a member of `group` would create a cycle: `nested ==
    /// group`, or `nested` is already reachable upward from `group` (a walk seeded at `group`
    /// reaches it -- i.e. `group` is already, directly or transitively, nested into `nested`).
    pub fn nesting_would_cycle(&self, group: &str, nested: &str) -> bool {
        if nested == group {
            return true;
        }
        self.walk_upward(BTreeSet::from([group.to_string()]), None)
            .contains(nested)
    }
}

/// Cache-TTL knob for [`DbGroupsSource`], read from env with a default.
#[derive(Clone, Copy, Debug)]
pub struct DbGroupsConfig {
    /// `MICROMEGAS_AUTH_CACHE_TTL_SECONDS`, default 60 -- a single flat, unprefixed knob (no
    /// `{prefix}_` role variant) shared with `DbApiKeyConfig`/`DbAudienceGrantsConfig`'s own
    /// positive-cache TTL: one value governs the API-key, audience-grant, and group snapshot
    /// caches process-wide, across every role. Membership and admin changes take effect within
    /// this TTL per process; that is the documented latency.
    pub cache_ttl_secs: u64,
}

impl DbGroupsConfig {
    /// Resolves the flat, unprefixed `MICROMEGAS_AUTH_CACHE_TTL_SECONDS` knob directly -- there
    /// is exactly one value for this knob process-wide, so `prefix` is accepted (for call-site
    /// symmetry with every other `from_env_with_prefix`) but not consulted here.
    pub fn from_env_with_prefix(_prefix: &str) -> Self {
        Self {
            cache_ttl_secs: resolve_u64("", "AUTH_CACHE_TTL_SECONDS", 60),
        }
    }
}

/// [`SnapshotLoader`] for the `groups`/`group_members` tables.
#[derive(Debug)]
pub struct GroupsLoader;

#[async_trait]
impl SnapshotLoader for GroupsLoader {
    type Snapshot = GroupGraph;
    const NAME: &'static str = "group store";

    /// Runs both `SELECT`s in one read transaction and builds a [`GroupGraph`], so a concurrent
    /// `DELETE ... CASCADE` cannot yield a snapshot with members of a group the `groups` query
    /// missed.
    async fn fetch(pool: &PgPool) -> Result<GroupGraph> {
        let mut tx = pool
            .begin()
            .await
            .context("starting group store read transaction")?;
        let group_rows = sqlx::query("SELECT name FROM groups")
            .fetch_all(&mut *tx)
            .await
            .context("querying groups")?;
        let mut names = Vec::with_capacity(group_rows.len());
        for row in group_rows {
            let name: String = row.try_get("name").context("reading name")?;
            names.push(name);
        }
        let member_rows = sqlx::query("SELECT group_name, member FROM group_members")
            .fetch_all(&mut *tx)
            .await
            .context("querying group_members")?;
        let mut members = Vec::with_capacity(member_rows.len());
        for row in member_rows {
            let group_name: String = row.try_get("group_name").context("reading group_name")?;
            let member: String = row.try_get("member").context("reading member")?;
            members.push((group_name, member));
        }
        tx.commit()
            .await
            .context("committing group store read transaction")?;
        GroupGraph::from_rows(names, members)
    }

    fn count_refresh_error() {
        micromegas_tracing::imetric!("group_refresh_error_count", "count", 1_u64);
    }
}

/// The whole-table snapshot cache described in the module doc comment.
pub type DbGroupsSource = SnapshotSource<GroupsLoader>;
