//! Default authentication provider initialization for Micromegas services.
//!
//! This module provides the standard way to initialize authentication with
//! API key, OIDC, and (optionally) a DB-backed API key store from environment
//! variables.

use crate::api_key::{ApiKeyAuthProvider, parse_key_ring};
use crate::db_api_key::{
    ApiKeyTable, DbApiKeyAuthProvider, DbApiKeyConfig, key_store_has_live_rows,
};
use crate::multi::MultiAuthProvider;
use crate::oidc::{OidcAuthProvider, OidcConfig};
use crate::types::AuthProvider;
use anyhow::Result;
use micromegas_tracing::{info, warn};
use sqlx::PgPool;
use std::sync::Arc;

/// Builder for the default (env-driven) authentication provider stack, plus an
/// optional DB-backed API key store.
///
/// The env factory (`provider()` / `provider_with_prefix()` below) is a builder
/// so that adding the DB store — and later a policy — does not re-break the
/// signature.
pub struct ProviderBuilder {
    prefix: String,
    key_store: Option<(PgPool, ApiKeyTable)>,
}

impl ProviderBuilder {
    /// Starts a builder scoped to `prefix` (e.g. `"MICROMEGAS_INGESTION"`, or `""`
    /// for the unprefixed default), following the same `{prefix}_*`-with-fallback
    /// convention as `provider_with_prefix`.
    pub fn new(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_string(),
            key_store: None,
        }
    }

    /// Attaches a DB-backed key store bound to `table`, looked up through `pool`
    /// (expected to be a [`crate::db_api_key::dedicated_key_store_pool`], not a
    /// clone of the caller's lake pool).
    pub fn with_db_key_store(mut self, pool: PgPool, table: ApiKeyTable) -> Self {
        self.key_store = Some((pool, table));
        self
    }

    /// Resolves the API-keys env var name for this builder's prefix, with
    /// fallback to the unprefixed name.
    fn api_keys_json(&self) -> Option<String> {
        if self.prefix.is_empty() {
            std::env::var("MICROMEGAS_API_KEYS").ok()
        } else {
            std::env::var(format!("{}_API_KEYS", self.prefix))
                .or_else(|_| std::env::var("MICROMEGAS_API_KEYS"))
                .ok()
        }
    }

    /// Resolves the OIDC config env var name for this builder's prefix, with
    /// fallback to the unprefixed name.
    fn oidc_config_var(&self) -> String {
        if self.prefix.is_empty() {
            "MICROMEGAS_OIDC_CONFIG".to_string()
        } else if std::env::var(format!("{}_OIDC_CONFIG", self.prefix)).is_ok() {
            format!("{}_OIDC_CONFIG", self.prefix)
        } else {
            "MICROMEGAS_OIDC_CONFIG".to_string()
        }
    }

    /// Resolves the admin-users env var name for this builder's prefix, with
    /// fallback to the unprefixed name.
    fn admin_var(&self) -> String {
        if self.prefix.is_empty() {
            "MICROMEGAS_ADMINS".to_string()
        } else {
            let prefixed = format!("{}_ADMINS", self.prefix);
            if std::env::var(&prefixed).is_ok() {
                prefixed
            } else {
                "MICROMEGAS_ADMINS".to_string()
            }
        }
    }

    /// Builds the composed provider.
    ///
    /// Composes, in this order: env `ApiKeyAuthProvider` (in-memory, cheapest,
    /// preserves today's precedence) → `OidcAuthProvider` → `DbApiKeyAuthProvider`.
    /// `MultiAuthProvider` tries providers in order, so putting the DB provider
    /// last means only tokens that are neither an env key nor a valid JWT ever
    /// reach it.
    ///
    /// **The DB provider is always pushed onto the chain whenever a key store is
    /// attached** — registration never depends on the existence query below, so a
    /// deployment that mints its first key through `POST /auth/api_keys` into a
    /// previously empty table authenticates it on the very next request, with no
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
    /// Returns `Ok(None)` when nothing is configured at all (preserving the
    /// "genuinely empty deployment" startup guard every caller relies on).
    pub async fn build(self) -> Result<Option<Arc<dyn AuthProvider>>> {
        let admin_var = self.admin_var();
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
                let oidc_provider = OidcAuthProvider::new(config, &admin_var).await?;
                multi = multi.with_provider(Arc::new(oidc_provider));
                configured = true;
            }
            Err(e) => {
                info!("OIDC not configured ({e}) - OIDC auth disabled");
            }
        }

        if let Some((pool, table)) = self.key_store {
            let db_config = DbApiKeyConfig::from_env_with_prefix(&self.prefix);
            let db_provider = DbApiKeyAuthProvider::new(pool.clone(), table, db_config);
            multi = multi.with_provider(Arc::new(db_provider));

            let has_live_rows = match key_store_has_live_rows(&pool, table).await {
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

        Ok(Some(Arc::new(multi) as Arc<dyn AuthProvider>))
    }
}

/// Initializes the default authentication provider with API key and OIDC from environment.
///
/// Reads configuration from:
/// - `MICROMEGAS_API_KEYS`: JSON array of API keys
/// - `MICROMEGAS_OIDC_CONFIG`: OIDC configuration JSON
/// - `MICROMEGAS_ADMINS`: JSON array of admin user emails/subjects
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
/// - Admins:   tries `MICROMEGAS_INGESTION_ADMINS`, falls back to `MICROMEGAS_ADMINS`
///
/// With an empty prefix the behaviour is identical to [`provider`]. Kept as a
/// thin env-only wrapper around [`ProviderBuilder`] — no DB key store is attached.
pub async fn provider_with_prefix(prefix: &str) -> Result<Option<Arc<dyn AuthProvider>>> {
    ProviderBuilder::new(prefix).build().await
}
