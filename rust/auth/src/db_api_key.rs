//! DB-backed API key authentication (#1383).
//!
//! Moves API keys out of `MICROMEGAS_API_KEYS` (plaintext JSON in an env var) into
//! two Postgres tables — `ingestion_api_keys` and `analytics_api_keys` (migration
//! v5, `rust/ingestion/src/sql_migration.rs`) — holding only a SHA-256 hash of each
//! key, plus a `created_at`/`created_by`/`last_used_at`/`revoked_at`/`revoked_by`
//! audit trail. [`DbApiKeyAuthProvider`] validates by hash-indexed lookup behind a
//! short-TTL `moka` cache.

use crate::types::{AuthContext, AuthProvider, AuthType, ProviderUnavailable, RequestParts};
use anyhow::{Context, Result, anyhow};
use base64::Engine;
use chrono::Utc;
use moka::future::Cache;
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use sqlx::Row;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

/// Which key table a provider (or the management API) is bound to.
///
/// The table name is always a `&'static str` literal — never derived from caller
/// input — so the SQL built from it (below) can never be injected into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiKeyTable {
    /// `ingestion_api_keys` — write credentials, minted/listed/revoked over HTTP.
    Ingestion,
    /// `analytics_api_keys` — read credentials, issued only by hand (see the admin
    /// runbook); never mintable through the HTTP API.
    Analytics,
}

impl ApiKeyTable {
    /// Static table name.
    pub fn table_name(self) -> &'static str {
        match self {
            ApiKeyTable::Ingestion => "ingestion_api_keys",
            ApiKeyTable::Analytics => "analytics_api_keys",
        }
    }
}

/// Cache and audit knobs for [`DbApiKeyAuthProvider`], read from env with defaults.
#[derive(Clone, Copy, Debug)]
pub struct DbApiKeyConfig {
    /// `MICROMEGAS_API_KEY_CACHE_SIZE`, default 10_000.
    pub cache_size: u64,
    /// `MICROMEGAS_API_KEY_CACHE_TTL_SECONDS`, default 60. Also the bound on
    /// revocation latency: a `DELETE` writes `revoked_at` but cannot invalidate a
    /// remote process's cache, so a revoked key keeps authenticating until this
    /// TTL elapses.
    pub cache_ttl_secs: u64,
    /// `MICROMEGAS_API_KEY_UNKNOWN_CACHE_TTL_SECONDS`, default 10. Shorter than
    /// `cache_ttl_secs` so a freshly minted key is not masked by an earlier probe
    /// of that same (not-yet-existing) string for longer than necessary.
    pub unknown_cache_ttl_secs: u64,
    /// `MICROMEGAS_API_KEY_UNKNOWN_CACHE_SIZE`, default 10_000.
    pub unknown_cache_size: u64,
}

fn resolve_u64(prefix: &str, suffix: &str, default: u64) -> u64 {
    let raw = if prefix.is_empty() {
        std::env::var(format!("MICROMEGAS_{suffix}")).ok()
    } else {
        std::env::var(format!("{prefix}_{suffix}"))
            .or_else(|_| std::env::var(format!("MICROMEGAS_{suffix}")))
            .ok()
    };
    raw.and_then(|s| s.parse::<u64>().ok()).unwrap_or(default)
}

impl DbApiKeyConfig {
    /// Resolves each of the four knobs as `{prefix}_API_KEY_CACHE_*` first, falling
    /// back to the unprefixed name — the same fallback `provider_with_prefix`
    /// already uses for `{prefix}_API_KEYS` / `{prefix}_OIDC_CONFIG` /
    /// `{prefix}_ADMINS`. With an empty prefix this is identical to the unprefixed
    /// vars, so an unprefixed caller just passes `""`.
    pub fn from_env_with_prefix(prefix: &str) -> Self {
        Self {
            cache_size: resolve_u64(prefix, "API_KEY_CACHE_SIZE", 10_000),
            cache_ttl_secs: resolve_u64(prefix, "API_KEY_CACHE_TTL_SECONDS", 60),
            unknown_cache_ttl_secs: resolve_u64(prefix, "API_KEY_UNKNOWN_CACHE_TTL_SECONDS", 10),
            unknown_cache_size: resolve_u64(prefix, "API_KEY_UNKNOWN_CACHE_SIZE", 10_000),
        }
    }
}

/// SHA-256 of the full key string. The only place a key is hashed; the mint route
/// (`rust/public/src/servers/api_keys.rs`) and this provider's lookup go through
/// the same digest definition.
pub fn hash_key(key: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hasher.finalize().into()
}

/// 256 bits of OS entropy, base64url-nopad, `mmk_`-prefixed. `mmk_` makes minted
/// keys recognizable to secret scanners; it is cosmetic to validation, since
/// [`hash_key`] covers the whole string (which is what lets imported legacy keys
/// of any shape keep working).
pub fn generate_key() -> String {
    let mut rng = rand::rng();
    let bytes: [u8; 32] = rng.random();
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    format!("mmk_{encoded}")
}

/// Builds a small, dedicated connection pool for key-store lookups from an
/// existing pool's connect options — rather than sharing the caller's lake pool
/// directly. `max_connections(4)`, `acquire_timeout(2s)`: a credential flood or a
/// DB outage on the key-lookup path can therefore never starve the write path of
/// connections, and a lookup fails fast into its 503 rather than blocking up to
/// sqlx's default 30s `acquire_timeout`.
pub fn dedicated_key_store_pool(lake_pool: &PgPool) -> PgPool {
    let options = (*lake_pool.connect_options()).clone();
    PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(2))
        .connect_lazy_with(options)
}

/// `SELECT EXISTS(SELECT 1 FROM <table> WHERE revoked_at IS NULL)` — used by
/// `ProviderBuilder::build()` (`rust/auth/src/default_provider.rs`) to decide
/// whether a non-empty key store counts as "auth configured" on its own. A
/// missing relation (e.g. a schema that has not reached migration v5 yet)
/// surfaces as an `Err` here and must be propagated, never treated as "empty".
pub async fn key_store_has_live_rows(pool: &PgPool, table: ApiKeyTable) -> Result<bool> {
    let row = sqlx::query(&format!(
        "SELECT EXISTS(SELECT 1 FROM {} WHERE revoked_at IS NULL) AS has_rows",
        table.table_name()
    ))
    .fetch_one(pool)
    .await
    .with_context(|| format!("checking whether {} has any live key", table.table_name()))?;
    let has_rows: bool = row
        .try_get("has_rows")
        .with_context(|| "reading has_rows")?;
    Ok(has_rows)
}

/// hash -> (key_id, name) for a key known to be live.
struct KeyRow {
    key_id: uuid::Uuid,
    name: String,
}

/// Distinguishes "the DB answered: no such live key" from "the DB could not be
/// reached at all" — only the latter becomes a [`ProviderUnavailable`] and must
/// never populate either cache.
#[derive(thiserror::Error, Debug)]
enum LookupError {
    #[error("no such live key")]
    NotFound,
    #[error("{0}")]
    Db(anyhow::Error),
}

fn table_tags(table: &'static str) -> &'static micromegas_tracing::property_set::PropertySet {
    micromegas_tracing::property_set::PropertySet::find_or_create(vec![
        micromegas_tracing::property_set::Property::new("table", table),
    ])
}

/// Rate-limits the outage `error!` log to at most once per `window_secs` (per
/// table), checked-and-set via a single `AtomicI64` "last logged at" timestamp —
/// not once per rejected request. §2's design notes: `DbApiKeyAuthProvider` sits
/// last in the auth chain, so during an outage every non-env-key, non-JWT request
/// reaches it; an unconditional `error!` would flood `log_entries` with the
/// outage's own noise on the highest-volume service in the deployment.
fn maybe_log_error(last_logged_at: &AtomicI64, window_secs: i64, table: &str, err: &anyhow::Error) {
    let now = Utc::now().timestamp();
    let prev = last_logged_at.load(Ordering::Relaxed);
    if now.saturating_sub(prev) >= window_secs
        && last_logged_at
            .compare_exchange(prev, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        micromegas_tracing::error!("db_api_key store error (table={table}): {err:#}");
    }
}

/// DB-backed API key authentication provider, bound to one [`ApiKeyTable`].
///
/// Every DB error is wrapped in [`ProviderUnavailable`] before being returned and
/// emits `imetric!("db_api_key_error_count", "count", {table}, 1)` unconditionally
/// — the metric is unconditional even on requests whose `error!` line was
/// suppressed by [`maybe_log_error`]'s rate limit.
pub struct DbApiKeyAuthProvider {
    pool: PgPool,
    table: ApiKeyTable,
    /// hash -> (key_id, name) for keys known to be live.
    valid: Cache<[u8; 32], Arc<KeyRow>>,
    /// hash -> () for tokens the DB answered "no such live key" for.
    unknown: Cache<[u8; 32], ()>,
    /// Rate-limit window for the outage `error!` log — mirrors `cache_ttl_secs`.
    error_log_window_secs: i64,
    last_logged_at: Arc<AtomicI64>,
}

impl DbApiKeyAuthProvider {
    /// Creates a new provider bound to `table`, using `pool` for lookups (the
    /// caller is expected to pass a [`dedicated_key_store_pool`], not the lake
    /// pool itself).
    pub fn new(pool: PgPool, table: ApiKeyTable, config: DbApiKeyConfig) -> Self {
        let valid = Cache::builder()
            .max_capacity(config.cache_size)
            .time_to_live(Duration::from_secs(config.cache_ttl_secs))
            .build();
        let unknown = Cache::builder()
            .max_capacity(config.unknown_cache_size)
            .time_to_live(Duration::from_secs(config.unknown_cache_ttl_secs))
            .build();
        Self {
            pool,
            table,
            valid,
            unknown,
            error_log_window_secs: config.cache_ttl_secs as i64,
            last_logged_at: Arc::new(AtomicI64::new(0)),
        }
    }
}

#[async_trait::async_trait]
impl AuthProvider for DbApiKeyAuthProvider {
    async fn validate_request(&self, parts: &dyn RequestParts) -> Result<AuthContext> {
        let token = parts
            .bearer_token()
            .ok_or_else(|| anyhow!("missing bearer token"))?;
        let hash = hash_key(token);

        // A repeated probe of the same bogus token is free after the first
        // attempt; a flood of distinct bogus tokens still costs one DB round trip
        // per request (see the plan's "A negative cache" trade-off).
        if self.unknown.get(&hash).await.is_some() {
            anyhow::bail!("invalid API token");
        }

        let pool = self.pool.clone();
        let table = self.table;
        let last_logged_at = self.last_logged_at.clone();
        let window_secs = self.error_log_window_secs;

        // `try_get_with`, not a plain `get`/`insert` pair: among concurrent
        // callers for the same hash, exactly one runs this loader — the rest
        // await its result. This is what rate-limits the `last_used_at` write to
        // once per cache TTL per key, not once per concurrent request.
        let result = self
            .valid
            .try_get_with(hash, async move {
                let row = sqlx::query(&format!(
                    "UPDATE {} SET last_used_at = now() WHERE key_hash = $1 AND revoked_at IS NULL RETURNING key_id, name",
                    table.table_name()
                ))
                .bind(&hash[..])
                .fetch_optional(&pool)
                .await
                .map_err(|e| {
                    let err = anyhow::Error::from(e)
                        .context(format!("looking up key in {}", table.table_name()));
                    // Unconditional: fires on every DB error, independent of the
                    // rate-limited `error!` line below.
                    micromegas_tracing::imetric!(
                        "db_api_key_error_count",
                        "count",
                        table_tags(table.table_name()),
                        1_u64
                    );
                    maybe_log_error(&last_logged_at, window_secs, table.table_name(), &err);
                    LookupError::Db(err)
                })?;

                match row {
                    Some(row) => {
                        let key_id: uuid::Uuid = row
                            .try_get("key_id")
                            .map_err(|e| LookupError::Db(anyhow::Error::from(e).context("reading key_id")))?;
                        let name: String = row
                            .try_get("name")
                            .map_err(|e| LookupError::Db(anyhow::Error::from(e).context("reading name")))?;
                        Ok(Arc::new(KeyRow { key_id, name }))
                    }
                    None => Err(LookupError::NotFound),
                }
            })
            .await;

        match result {
            Ok(row) => {
                micromegas_tracing::trace!(
                    "db api key validated: table={} key_id={} name={}",
                    self.table.table_name(),
                    row.key_id,
                    row.name
                );
                Ok(AuthContext {
                    subject: row.name.clone(),
                    email: None,
                    issuer: "api_key".to_string(),
                    audience: None,
                    expires_at: None,
                    auth_type: AuthType::ApiKey,
                    // SECURITY: API keys can NEVER be admins.
                    is_admin: false,
                    allow_delegation: true,
                })
            }
            // A DB *error* is propagated and cached in neither map: caching an
            // outage as `unknown` would turn a transient failure into a
            // TTL-long outage for every affected key.
            Err(arc_err) => match arc_err.as_ref() {
                LookupError::NotFound => {
                    self.unknown.insert(hash, ()).await;
                    Err(anyhow!("invalid API token"))
                }
                LookupError::Db(e) => Err(ProviderUnavailable(anyhow!(
                    "{} key store unavailable: {e:#}",
                    self.table.table_name()
                ))
                .into()),
            },
        }
    }
}
