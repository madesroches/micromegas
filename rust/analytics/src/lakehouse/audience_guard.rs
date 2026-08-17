//! Query Enforcement Prong B (#1371, AbAC Stage 3) -- arg-addressed guards for the span/metadata
//! UDTFs and the `get_payload` UDF that [`super::ownership_rewrite::OwnershipRewrite`] (Prong A)
//! structurally cannot reach: they bake their target id into a provider at plan time, return
//! schemas with no `process_id` column to filter on, and some build their own inner session
//! under `ReadScope::All`. See `tasks/1371_udtf_udf_guards_plan.md` for the full design rationale;
//! this comment records only what a future reader of this file needs close at hand.
//!
//! ## One cache, one question
//!
//! [`AudienceIndex`] answers exactly one question, for three id kinds ([`IdKind`]): which
//! audience is stamped on the process that owns this id? It resolves from **Postgres** (the
//! origin of `micromegas.audience`), by primary-key point query -- fresher and independent of
//! materialization, unlike [`super::ownership_rewrite::OwnershipRewrite`], which reads a
//! daemon-materialized snapshot (see the plan's §11 for the consequences of the two prongs
//! reading different copies).
//!
//! ## Fail-closed
//!
//! [`is_readable`] is the whole authorization rule, pure and offline-testable: `ReadScope::All`
//! passes everything; `ReadScope::Audiences` denies [`OwnerAudience::Unknown`] unconditionally,
//! passes [`OwnerAudience::Unstamped`] only when `unstamped_audience` is both configured and in
//! scope, and matches [`OwnerAudience::Audience`] byte-exactly. An id ambiguous between a
//! `process_id` and a `stream_id` interpretation ([`OwnerAudience::Ambiguous`]) is readable only
//! when every interpretation is -- never by picking one arm over the other. A resolution *error*
//! (Postgres unreachable) is a denial too -- [`AudienceGuard::authorize`]/
//! [`AudienceGuard::readable_ids`] map it to a query failure, never to a readable verdict.
//!
//! ## No existence oracle
//!
//! Every guard denial and every "no such id" produce the same error text (e.g. `process_spans:
//! 'xxx' not found or not accessible`) -- a distinct "permission denied" would let a caller
//! enumerate which ids exist in other audiences. The server log ([`debug!`]) records the real
//! reason so an operator can tell the two apart; the client cannot.

use super::read_scope::{CallerContext, ReadScope};
use anyhow::Context;
use datafusion::common::plan_err;
use datafusion::error::DataFusionError;
use micromegas_tracing::prelude::*;
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// The process property carrying the data-isolation audience. Single definition, shared with
/// [`super::ownership_rewrite::OwnershipRewrite`] (which reads it via [`Self::audience_col`] --
/// formerly its own inlined literal, before Prong B introduced this shared constant).
pub const AUDIENCE_PROPERTY: &str = "micromegas.audience";

/// What a resolution attempt found. `Unknown` is *not* `Unstamped`: an id with no row at all is
/// always denied, while an unstamped process is subject to the `unstamped_audience` knob.
/// "No row" covers both "not yet ingested" and "retention already deleted it" (plan §11) -- both
/// deny, on the same fail-closed reasoning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnerAudience {
    Unknown,
    Unstamped,
    Audience(Arc<str>),
    /// The id resolved to more than one distinct owner under `IdKind::ProcessOrStream`'s two
    /// arms -- a `process_id`/`stream_id` collision (see the variant's doc comment on
    /// [`IdKind::ProcessOrStream`]). Fail-closed: [`is_readable`] treats this as readable only
    /// when *every* one of the distinct owners would independently be readable, never by picking
    /// one arm over the other.
    Ambiguous(Vec<OwnerAudience>),
}

/// Which table resolves the id to its owning process. Also the cache's key discriminator: the
/// same `Uuid` can legitimately be a `process_id` in one audience and a `stream_id`/`block_id` in
/// another (all three are client-supplied at ingestion, with no cross-table uniqueness
/// constraint), so caching by kind keeps those disjoint instead of relying on downstream
/// emptiness to make a wrong-kind hit harmless.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum IdKind {
    /// `processes.process_id`.
    Process,
    /// `blocks.block_id` -> `blocks.process_id` -> `processes`.
    Block,
    /// `list_partitions`' `view_instance_id`: either a `process_id` or a `stream_id`, resolved in
    /// one round trip. Cached under its own key rather than reusing `Process`/`Block` entries or a
    /// separate `streams` kind, so the `UNION ALL` result -- fail-closed on a collision between the
    /// two arms, see [`merge_owner_rows`] -- is what actually gets cached.
    ProcessOrStream,
}

/// Default entry-count bound for [`AudienceIndex`]'s cache -- see [`super::lakehouse_context`].
/// One `Uuid` + a short audience string is roughly 100 bytes, so 100k entries is ~10 MB: a fixed
/// shape with no operational knob, mirroring `analytics-web-srv/src/data_source_cache.rs`'s
/// hardcoded `.max_capacity(1000)` rather than `MICROMEGAS_METADATA_CACHE_MB`'s per-entry-weight
/// variability.
pub const DEFAULT_AUDIENCE_CACHE_ENTRIES: u64 = 100_000;

/// Default freshness bound for [`AudienceIndex`]'s cache entries. The *only* invalidation
/// mechanism: a process's Postgres row is written once and never updated in place, but a
/// retention-then-re-export cycle can recreate the same `process_id` (deterministic UUIDv5 for
/// OTLP) under fresh `properties`, and nothing else ever expires a stale cached answer. Bounds
/// how long a re-derived process's audience can serve a stale answer.
pub const DEFAULT_AUDIENCE_CACHE_TTL: Duration = Duration::from_secs(5 * 60);

/// One row of the id -> owning-process-audience resolution, tagged with which arm of the
/// (possibly `UNION ALL`) query produced it. Only `ProcessOrStream`'s query can produce both tags
/// for the same id (a `process_id`/`stream_id` collision); `Process`/`Block` only ever produce
/// `Process`-tagged rows. Kept only for `debug!` diagnostics in [`merge_owner_rows`] -- it no
/// longer picks a winner between the two arms.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum OwnerSource {
    Process,
    Stream,
}

/// Merges possibly-duplicated `(id, audience, source)` rows into one [`OwnerAudience`] per id.
/// Fail-closed on a collision: when `ProcessOrStream`'s two arms resolve the same id to
/// *different* audiences (a `process_id`/`stream_id` collision -- both ids are client-supplied at
/// ingestion, with no cross-table uniqueness constraint), neither arm wins over the other --
/// the id maps to [`OwnerAudience::Ambiguous`], which [`is_readable`] only passes when every
/// resolved audience is independently readable. `Process`/`Block` queries never produce more than
/// one row per id, so this never triggers for them.
fn merge_owner_rows(
    rows: Vec<(Uuid, Option<String>, OwnerSource)>,
) -> HashMap<Uuid, OwnerAudience> {
    let mut by_id: HashMap<Uuid, Vec<OwnerAudience>> = HashMap::new();
    for (id, audience, _source) in rows {
        let owner = match audience {
            Some(a) => OwnerAudience::Audience(Arc::from(a)),
            None => OwnerAudience::Unstamped,
        };
        let distinct_owners = by_id.entry(id).or_default();
        if !distinct_owners.contains(&owner) {
            distinct_owners.push(owner);
        }
    }
    by_id
        .into_iter()
        .map(|(id, mut owners)| {
            let owner = if owners.len() == 1 {
                owners.pop().expect("checked len() == 1 above")
            } else {
                debug!(
                    "audience_guard: '{id}' resolved to {} distinct owners across process_id/stream_id \
                     collision, treating as Ambiguous (fail-closed)",
                    owners.len()
                );
                OwnerAudience::Ambiguous(owners)
            };
            (id, owner)
        })
        .collect()
}

/// The SQL shape for each [`IdKind`]. `LEFT JOIN LATERAL` (not an inner `unnest` in the `FROM`
/// list) is what keeps *unstamped* rows in the result -- an inner unnest would silently drop
/// them, collapsing `Unstamped` into `Unknown`. `$2` is [`AUDIENCE_PROPERTY`]; `$1` is the batch
/// of ids to resolve, bound as a `uuid[]` array so `resolve_many` is always one query.
fn owner_query_sql(kind: IdKind) -> &'static str {
    match kind {
        IdKind::Process => {
            "SELECT p.process_id AS id, a.value AS audience, 'process' AS source
             FROM processes p
             LEFT JOIN LATERAL (
                 SELECT value FROM unnest(p.properties) WHERE key = $2 LIMIT 1
             ) a ON TRUE
             WHERE p.process_id = ANY($1::uuid[])"
        }
        IdKind::Block => {
            "SELECT b.block_id AS id, a.value AS audience, 'process' AS source
             FROM blocks b
             JOIN processes p ON p.process_id = b.process_id
             LEFT JOIN LATERAL (
                 SELECT value FROM unnest(p.properties) WHERE key = $2 LIMIT 1
             ) a ON TRUE
             WHERE b.block_id = ANY($1::uuid[])"
        }
        IdKind::ProcessOrStream => {
            "SELECT p.process_id AS id, a.value AS audience, 'process' AS source
             FROM processes p
             LEFT JOIN LATERAL (
                 SELECT value FROM unnest(p.properties) WHERE key = $2 LIMIT 1
             ) a ON TRUE
             WHERE p.process_id = ANY($1::uuid[])
             UNION ALL
             SELECT s.stream_id AS id, a.value AS audience, 'stream' AS source
             FROM streams s
             JOIN processes p ON p.process_id = s.process_id
             LEFT JOIN LATERAL (
                 SELECT value FROM unnest(p.properties) WHERE key = $2 LIMIT 1
             ) a ON TRUE
             WHERE s.stream_id = ANY($1::uuid[])"
        }
    }
}

async fn fetch_owner_rows(
    pool: &sqlx::Pool<sqlx::Postgres>,
    ids: &[Uuid],
    kind: IdKind,
) -> anyhow::Result<Vec<(Uuid, Option<String>, OwnerSource)>> {
    let rows = sqlx::query(owner_query_sql(kind))
        .bind(ids)
        .bind(AUDIENCE_PROPERTY)
        .fetch_all(pool)
        .await
        .with_context(|| format!("resolving owning process' audience for {kind:?}"))?;
    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id").context("reading id column")?;
            let audience: Option<String> =
                row.try_get("audience").context("reading audience column")?;
            let source: String = row.try_get("source").context("reading source column")?;
            let source = if source == "process" {
                OwnerSource::Process
            } else {
                OwnerSource::Stream
            };
            Ok((id, audience, source))
        })
        .collect()
}

/// Resolves *any telemetry id -> its owning process's audience*, from Postgres, cached and
/// TTL-bounded. See the module doc comment for the full rationale.
pub struct AudienceIndex {
    pool: sqlx::Pool<sqlx::Postgres>,
    cache: moka::future::Cache<(IdKind, Uuid), OwnerAudience>,
}

impl AudienceIndex {
    pub fn new(pool: sqlx::Pool<sqlx::Postgres>, max_entries: u64, ttl: Duration) -> Self {
        let cache = moka::future::Cache::builder()
            .max_capacity(max_entries)
            .time_to_live(ttl)
            .build();
        Self { pool, cache }
    }

    /// Resolves a single id. A thin wrapper over [`Self::resolve_many`] -- always at least as
    /// cheap, and the batch path is where the actual query lives.
    pub async fn resolve(&self, id: Uuid, kind: IdKind) -> anyhow::Result<OwnerAudience> {
        let mut resolved = self.resolve_many(&[id], kind).await?;
        Ok(resolved.remove(&id).unwrap_or(OwnerAudience::Unknown))
    }

    /// Resolves a batch of ids of the same `kind` in at most one round trip (cache misses only).
    /// `Unknown` is never cached -- a miss may just mean "not ingested yet", and caching it would
    /// both pin a wrong answer and let a caller pollute the cache with random ids.
    pub async fn resolve_many(
        &self,
        ids: &[Uuid],
        kind: IdKind,
    ) -> anyhow::Result<HashMap<Uuid, OwnerAudience>> {
        let mut result = HashMap::with_capacity(ids.len());
        let mut misses: Vec<Uuid> = Vec::new();
        let mut seen_miss: HashSet<Uuid> = HashSet::new();
        for &id in ids {
            if let Some(cached) = self.cache.get(&(kind, id)).await {
                result.insert(id, cached);
            } else if seen_miss.insert(id) {
                misses.push(id);
            }
        }
        if misses.is_empty() {
            return Ok(result);
        }
        let rows = fetch_owner_rows(&self.pool, &misses, kind).await?;
        let resolved = merge_owner_rows(rows);
        for id in misses {
            let owner = resolved.get(&id).cloned().unwrap_or(OwnerAudience::Unknown);
            if owner != OwnerAudience::Unknown {
                self.cache.insert((kind, id), owner.clone()).await;
            }
            result.insert(id, owner);
        }
        imetric!(
            "audience_cache_entry_count",
            "count",
            self.cache.entry_count()
        );
        Ok(result)
    }
}

impl std::fmt::Debug for AudienceIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudienceIndex")
            .field("entries", &self.cache.entry_count())
            .finish()
    }
}

/// Pure, offline-testable: the whole authorization rule, with no I/O in it. `ReadScope::All` is
/// the only branch that passes `Unknown` -- every other combination denies it unconditionally.
pub fn is_readable(
    scope: &ReadScope,
    unstamped_audience: Option<&str>,
    owner: &OwnerAudience,
) -> bool {
    match scope {
        ReadScope::All => true,
        ReadScope::Audiences(auds) => match owner {
            OwnerAudience::Unknown => false,
            OwnerAudience::Unstamped => {
                unstamped_audience.is_some_and(|u| auds.iter().any(|a| a == u))
            }
            OwnerAudience::Audience(a) => auds.iter().any(|x| x.as_str() == &**a),
            OwnerAudience::Ambiguous(owners) => owners
                .iter()
                .all(|owner| is_readable(scope, unstamped_audience, owner)),
        },
    }
}

/// Witness that some id's owning process was resolved and found readable under the caller's
/// scope. Constructible only by [`AudienceGuard::authorize`] -- a future call site that wants an
/// inner, unscoped session must hold one of these first, which is what keeps a fourth such UDTF
/// from silently reproducing the bypass this stage closes by calling `CallerContext::internal()`
/// directly. Not airtight: `internal()` stays public for genuinely non-user-reachable callers.
#[derive(Debug)]
pub struct Authorized {
    id: Uuid,
}

impl Authorized {
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// The `CallerContext` an inner, post-authorization session may run under. Deliberately
    /// `ReadScope::All` (not the caller's own scope) -- see
    /// `tasks/1371_udtf_udf_guards_plan.md` §6 for why guard-then-internal is the chosen shape
    /// here rather than scope inheritance: every inner statement these three call sites run is
    /// server-constructed and confined to `self.id`, so if `self.id`'s process is readable,
    /// everything those statements can reach is readable too.
    pub fn internal_caller(&self) -> CallerContext {
        CallerContext::internal()
    }
}

/// The whole guard: pure decision (`is_readable`) plus async resolution (`AudienceIndex`),
/// fail-closed throughout. One instance is built per request (in `query.rs`'s
/// `register_lakehouse_functions`) and shared, via `Arc`, across every arg-addressed UDTF/UDF the
/// request's session registers.
#[derive(Debug)]
pub struct AudienceGuard {
    read_scope: ReadScope,
    unstamped_audience: Option<String>,
    public_view_sets: Vec<String>,
    index: Arc<AudienceIndex>,
}

impl AudienceGuard {
    pub fn new(
        read_scope: ReadScope,
        unstamped_audience: Option<String>,
        public_view_sets: Vec<String>,
        index: Arc<AudienceIndex>,
    ) -> Self {
        Self {
            read_scope,
            unstamped_audience,
            public_view_sets,
            index,
        }
    }

    /// The caller's scope. Exposed so callers with their own row-level filtering
    /// (`list_partitions`) can decide up front whether to even attempt an optimization (e.g.
    /// `LIMIT` pushdown) that's only valid under `ReadScope::All`.
    pub fn read_scope(&self) -> &ReadScope {
        &self.read_scope
    }

    /// `ReadScope::All` ⇒ `Ok` with no I/O at all. Otherwise resolves `id`'s owning process's
    /// audience and denies with a uniform, existence-oracle-proof message
    /// (`{fname}: '{id}' not found or not accessible`) on anything but a readable verdict -- a
    /// resolution error included, never a pass. The real reason (resolved `OwnerAudience`, the
    /// caller's scope) is logged at `debug!`, for operators only.
    pub async fn authorize(
        &self,
        id: Uuid,
        kind: IdKind,
        fname: &str,
    ) -> datafusion::error::Result<Authorized> {
        if self.read_scope == ReadScope::All {
            return Ok(Authorized { id });
        }
        let owner = match self.index.resolve(id, kind).await {
            Ok(owner) => owner,
            Err(e) => {
                debug!("{fname}: audience resolution failed for '{id}' ({kind:?}): {e:#}");
                return Err(DataFusionError::External(e.into()));
            }
        };
        if is_readable(&self.read_scope, self.unstamped_audience.as_deref(), &owner) {
            Ok(Authorized { id })
        } else {
            debug!(
                "{fname}: denying '{id}' ({kind:?}): owner={owner:?}, scope={:?}",
                self.read_scope
            );
            plan_err!("{fname}: '{id}' not found or not accessible")
        }
    }

    /// Batch form of the same rule, for the callers that row-filter (`list_partitions`) or need
    /// an all-or-nothing verdict over a batch (`get_payload`) rather than a single witness.
    /// `ReadScope::All` ⇒ every id is readable, with no I/O.
    pub async fn readable_ids(
        &self,
        ids: &[Uuid],
        kind: IdKind,
    ) -> datafusion::error::Result<HashSet<Uuid>> {
        if self.read_scope == ReadScope::All {
            return Ok(ids.iter().copied().collect());
        }
        let owners = self
            .index
            .resolve_many(ids, kind)
            .await
            .map_err(|e| DataFusionError::External(e.into()))?;
        Ok(ids
            .iter()
            .copied()
            .filter(|id| {
                let owner = owners.get(id).unwrap_or(&OwnerAudience::Unknown);
                is_readable(&self.read_scope, self.unstamped_audience.as_deref(), owner)
            })
            .collect())
    }

    /// `list_partitions`' `'global'`-row rule (plan §8): a global aggregate has no single
    /// audience, so it is treated like unstamped data -- visible under `ReadScope::All`, when
    /// `view_set_name` is on the public allowlist, or when the configured `unstamped_audience` is
    /// itself in the caller's scope.
    pub fn global_rows_visible(&self, view_set_name: &str) -> bool {
        match &self.read_scope {
            ReadScope::All => true,
            ReadScope::Audiences(auds) => {
                self.public_view_sets.iter().any(|s| s == view_set_name)
                    || self
                        .unstamped_audience
                        .as_deref()
                        .is_some_and(|u| auds.iter().any(|a| a == u))
            }
        }
    }
}
