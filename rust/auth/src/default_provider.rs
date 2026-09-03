//! Default authentication provider initialization for Micromegas services.
//!
//! This module provides the standard way to initialize authentication with
//! API key, OIDC, and (optionally) a DB-backed API key store from environment
//! variables.

use crate::api_key::{ApiKeyAuthProvider, parse_key_ring};
use crate::db_api_key::{
    ApiKeyTable, DbApiKeyAuthProvider, DbApiKeyConfig, key_store_has_live_rows,
};
use crate::env::resolve_prefixed_var;
use crate::groups::{DbGroupsConfig, DbGroupsSource};
use crate::membership::MembershipProvider;
use crate::multi::MultiAuthProvider;
use crate::oidc::{OidcAuthProvider, OidcConfig};
use crate::types::AuthProvider;
use anyhow::Result;
use micromegas_tracing::{info, warn};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;

/// Builder for the default (env-driven) authentication provider stack, plus an
/// optional DB-backed API key store and an optional local-group store.
///
/// The env factory (`provider()` / `provider_with_prefix()` below) is a builder
/// so that adding the DB store — and later a policy — does not re-break the
/// signature.
pub struct ProviderBuilder {
    prefix: String,
    key_store: Option<(PgPool, ApiKeyTable)>,
    group_store: Option<PgPool>,
}

impl ProviderBuilder {
    /// Starts a builder scoped to `prefix` (e.g. `"MICROMEGAS_INGESTION"`, or `""`
    /// for the unprefixed default), following the same `{prefix}_*`-with-fallback
    /// convention as `provider_with_prefix`.
    pub fn new(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_string(),
            key_store: None,
            group_store: None,
        }
    }

    /// Attaches a DB-backed key store bound to `table`, looked up through `pool`
    /// (expected to be a [`crate::db_api_key::dedicated_key_store_pool`], not a
    /// clone of the caller's lake pool).
    pub fn with_db_key_store(mut self, pool: PgPool, table: ApiKeyTable) -> Self {
        self.key_store = Some((pool, table));
        self
    }

    /// Attaches a local-group store, looked up through `pool` (same dedicated-pool
    /// expectation as [`Self::with_db_key_store`]). `compose()` wraps the finished chain in a
    /// [`MembershipProvider`] when this is attached.
    pub fn with_group_store(mut self, pool: PgPool) -> Self {
        self.group_store = Some(pool);
        self
    }

    /// Resolves the API-keys env var name for this builder's prefix, with
    /// fallback to the unprefixed name.
    fn api_keys_json(&self) -> Option<String> {
        std::env::var(resolve_prefixed_var(&self.prefix, "API_KEYS")).ok()
    }

    /// Resolves the OIDC config env var name for this builder's prefix, with
    /// fallback to the unprefixed name.
    fn oidc_config_var(&self) -> String {
        resolve_prefixed_var(&self.prefix, "OIDC_CONFIG")
    }

    /// Composes the chain and reports whether env keys or OIDC counted as
    /// "configured". Shared by `build()` and `build_chain()`, which each
    /// document the provider order and the DB-provider guarantee.
    ///
    /// Takes `&self` rather than `self` so `build()` can still reach the
    /// attached key store's pool for its existence query afterwards.
    async fn compose(&self) -> Result<(MultiAuthProvider, bool)> {
        let oidc_config_var = self.oidc_config_var();
        let api_keys_json = self.api_keys_json();

        let mut multi = MultiAuthProvider::new();
        let mut configured = false;

        if let Some(keys_json) = api_keys_json {
            let keyring = parse_key_ring(&keys_json)?;
            info!("API key authentication enabled");
            multi = multi.with_provider(Arc::new(ApiKeyAuthProvider::new(keyring)));
            configured = true;
        } else {
            info!("API key auth not configured");
        }

        match OidcConfig::from_env_var(&oidc_config_var) {
            Ok(config) => {
                info!("Initializing OIDC authentication");
                let oidc_provider = OidcAuthProvider::new(config).await?;
                multi = multi.with_provider(Arc::new(oidc_provider));
                configured = true;
            }
            Err(e) => {
                info!("OIDC not configured ({e}) - OIDC auth disabled");
            }
        }

        if let Some((pool, table)) = &self.key_store {
            let db_config = DbApiKeyConfig::from_env_with_prefix(&self.prefix);
            let db_provider = DbApiKeyAuthProvider::new(pool.clone(), *table, db_config);
            multi = multi.with_provider(Arc::new(db_provider));
        }

        Ok((multi, configured))
    }

    /// Wraps `multi` in a [`MembershipProvider`] when a group store is attached, and runs one
    /// eager `current()` at startup: on `Ok`, `warn!`s while `has_wildcard_admin()` is true
    /// ("every authenticated caller is an admin; add a `user:` member to `admins` and remove
    /// `*`"); on `Err`, `warn!`s and continues -- the first request will 503, and a split
    /// deployment may legitimately start flight-sql before the migration runner is up, as with
    /// the v5 key store.
    async fn wrap_with_membership(&self, multi: MultiAuthProvider) -> Arc<dyn AuthProvider> {
        let Some(pool) = &self.group_store else {
            return Arc::new(multi) as Arc<dyn AuthProvider>;
        };
        let group_config = DbGroupsConfig::from_env_with_prefix(&self.prefix);
        let groups = Arc::new(DbGroupsSource::new(
            pool.clone(),
            Duration::from_secs(group_config.cache_ttl_secs),
        ));
        match groups.current().await {
            Ok(graph) => {
                if graph.has_wildcard_admin() {
                    warn!(
                        "every authenticated caller is an admin; add a `user:` member to \
                         `admins` and remove `*`"
                    );
                }
            }
            Err(e) => {
                warn!("group store not yet reachable at startup, continuing: {e:#}");
            }
        }
        Arc::new(MembershipProvider::new(
            Arc::new(multi) as Arc<dyn AuthProvider>,
            groups,
        )) as Arc<dyn AuthProvider>
    }

    /// Builds the composed provider.
    ///
    /// Composes, in order: env `ApiKeyAuthProvider` → `OidcAuthProvider` →
    /// `DbApiKeyAuthProvider`; `MultiAuthProvider` tries providers in order, so
    /// putting the DB provider last means only tokens that are neither an env
    /// key nor a valid JWT ever reach it. The DB provider is always pushed onto
    /// the chain whenever a key store is attached, so a key minted into a
    /// previously empty table authenticates on the very next request, with no
    /// restart.
    ///
    /// **A DB key store with at least one live key counts as "auth configured";
    /// an empty one does not.** When a key store is attached, this runs one cheap
    /// startup existence query (`key_store_has_live_rows`) and treats a non-empty
    /// result the same as env keys or OIDC being present. A failure of that query
    /// (e.g. a missing relation because the schema has not reached migration v5
    /// yet) is propagated as an `Err` from `build()` — unless env keys or OIDC
    /// already configured auth, in which case the failure is only `warn!`-logged
    /// and `has_live_rows` is treated as `false`, since the query's result would
    /// be unused either way.
    ///
    /// The chain is discarded when nothing counted as configured; use
    /// `build_chain()` to skip the guard.
    pub async fn build(self) -> Result<Option<Arc<dyn AuthProvider>>> {
        let (multi, mut configured) = self.compose().await?;

        if let Some((pool, table)) = &self.key_store {
            let has_live_rows = match key_store_has_live_rows(pool, *table).await {
                Ok(has_live_rows) => has_live_rows,
                Err(e) if !configured => {
                    return Err(e.context(format!(
                        "checking whether {} has any live key — has the schema reached migration v5? \
                         (rust/ingestion/src/sql_migration.rs; the ingestion binary or monolith must run \
                         the migration before flight-sql starts in a split deployment)",
                        table.table_name()
                    )));
                }
                Err(e) => {
                    // Auth is already configured via another provider (env keys or
                    // OIDC); this query's only purpose is deciding whether an
                    // otherwise-unconfigured deployment counts as configured, so a
                    // failure here (e.g. schema not yet at v5) must not abort
                    // startup.
                    warn!(
                        "checking whether {} has any live key failed, ignoring (auth already configured): {e:#}",
                        table.table_name()
                    );
                    false
                }
            };
            if has_live_rows {
                configured = true;
            }
        }

        if !configured {
            return Ok(None);
        }

        Ok(Some(self.wrap_with_membership(multi).await))
    }

    /// Composes the provider chain with **no** "is anything configured?" guard.
    ///
    /// Composes, in order: env `ApiKeyAuthProvider` → `OidcAuthProvider` →
    /// `DbApiKeyAuthProvider`. The DB provider is always pushed onto the chain
    /// whenever a key store is attached, so a key minted into a previously
    /// empty table authenticates on the very next request, with no restart.
    ///
    /// Always returns the chain — never `None`, because there is no `Option` —
    /// and never runs `key_store_has_live_rows`, so unlike `build()` it cannot
    /// fail on a schema short of migration v5. This is the entry point for a
    /// caller that wants the no-restart property above without inheriting
    /// `build()`'s startup guard or its existence-query failure mode — e.g. an
    /// embedder folding this chain into a larger `MultiAuthProvider` via
    /// `FlightSqlServer::with_auth_provider`.
    ///
    /// When nothing at all is configured (no env keys, no OIDC, no key store)
    /// the returned chain is an empty `MultiAuthProvider`, which rejects every
    /// request — fail-closed, since the caller asked for no guard. Logs a
    /// `warn!` in that case, since an empty chain is useless to any caller.
    pub async fn build_chain(self) -> Result<Arc<dyn AuthProvider>> {
        let (multi, _) = self.compose().await?;
        if multi.is_empty() {
            warn!("no auth provider configured: env keys, OIDC, and DB key store are all absent");
        }
        Ok(self.wrap_with_membership(multi).await)
    }
}

/// Initializes the default authentication provider with API key and OIDC from environment.
///
/// Reads configuration from:
/// - `MICROMEGAS_API_KEYS`: JSON array of API keys
/// - `MICROMEGAS_OIDC_CONFIG`: OIDC configuration JSON
///
/// Admin-ness is no longer an env-driven list here: it lives in the `admins` group (schema v10),
/// resolved by a [`crate::membership::MembershipProvider`] when a group store is attached via
/// [`ProviderBuilder::with_group_store`].
///
/// Returns `Ok(Some(...))` if at least one provider is configured.
/// Returns `Ok(None)` if no providers are configured (auth disabled).
/// Returns `Err` on configuration errors.
///
/// Kept as a thin env-only wrapper around [`ProviderBuilder`] — this crate's
/// documented, published entry point for a caller that wants API-key + OIDC
/// composition without a DB pool.
///
/// # Example
///
/// ```rust,no_run
/// use micromegas_auth::default_provider::provider;
///
/// # async fn example() -> anyhow::Result<()> {
/// let auth_provider = provider().await?;
/// if let Some(provider) = auth_provider {
///     println!("Authentication enabled");
/// } else {
///     println!("No authentication configured");
/// }
/// # Ok(())
/// # }
/// ```
pub async fn provider() -> Result<Option<Arc<dyn AuthProvider>>> {
    provider_with_prefix("").await
}

/// Initializes auth providers using env vars scoped to a prefix.
///
/// For prefix `"MICROMEGAS_INGESTION"`:
/// - API keys: tries `MICROMEGAS_INGESTION_API_KEYS`, falls back to `MICROMEGAS_API_KEYS`
/// - OIDC:     tries `MICROMEGAS_INGESTION_OIDC_CONFIG`, falls back to `MICROMEGAS_OIDC_CONFIG`
///
/// With an empty prefix the behaviour is identical to [`provider`]. Kept as a
/// thin env-only wrapper around [`ProviderBuilder`] — no DB key store is attached.
pub async fn provider_with_prefix(prefix: &str) -> Result<Option<Arc<dyn AuthProvider>>> {
    ProviderBuilder::new(prefix).build().await
}
