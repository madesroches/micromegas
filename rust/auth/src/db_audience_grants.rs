//! DB-backed audience grant store: a whole-table snapshot cache over the `audience_grants` table
//! (migration v7, `rust/ingestion/src/sql_migration.rs`), checked alongside the existing
//! `{prefix}_AUDIENCE_GRANTS` env map by
//! [`crate::policy::AudienceReadPolicy`]/[`crate::policy::AudienceMintPolicy`] -- a selector
//! present in either source grants access, without either side being deep-cloned or merged into
//! a combined map (`current()` hands back the cached grants behind an `Arc`). This is what makes
//! a grant creatable without a service restart -- the env map stays the static/bootstrap layer.
//!
//! The cache mechanics (cold-start throttling, last-good serving, `ProviderUnavailable`
//! wrapping) live in [`crate::db_snapshot`], shared with [`crate::groups::DbGroupsSource`].

use crate::db_api_key::resolve_u64;
use crate::db_snapshot::{SnapshotLoader, SnapshotSource};
use crate::policy::{AudienceGrants, GrantAxis};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use sqlx::{PgPool, Row};

/// Cache-TTL knob for [`DbAudienceGrantsSource`], read from env with a default.
#[derive(Clone, Copy, Debug)]
pub struct DbAudienceGrantsConfig {
    /// `MICROMEGAS_AUTH_CACHE_TTL_SECONDS`, default 60 -- a single flat, unprefixed knob (no
    /// `{prefix}_` role variant) shared with `DbApiKeyConfig`/`DbGroupsConfig`'s own
    /// positive-cache TTL: one value governs the API-key, audience-grant, and group snapshot
    /// caches process-wide, across every role.
    pub cache_ttl_secs: u64,
}

impl DbAudienceGrantsConfig {
    /// Resolves the flat, unprefixed `MICROMEGAS_AUTH_CACHE_TTL_SECONDS` knob directly -- there
    /// is exactly one value for this knob process-wide, so `prefix` is accepted (for call-site
    /// symmetry with every other `from_env_with_prefix`) but not consulted here.
    pub fn from_env_with_prefix(_prefix: &str) -> Self {
        Self {
            cache_ttl_secs: resolve_u64("", "AUTH_CACHE_TTL_SECONDS", 60),
        }
    }
}

/// [`SnapshotLoader`] for the `audience_grants` table.
#[derive(Debug)]
pub struct AudienceGrantsLoader;

#[async_trait]
impl SnapshotLoader for AudienceGrantsLoader {
    type Snapshot = AudienceGrants;
    const NAME: &'static str = "audience grant store";

    /// Queries the whole table and builds an `AudienceGrants` via
    /// [`AudienceGrants::from_rows`]. The one place a malformed row (one that slipped past the
    /// table's own `CHECK` constraints, e.g. via a direct `psql` session) surfaces as a load
    /// failure rather than a silently-inert or silently-unreadable grant.
    async fn fetch(pool: &PgPool) -> Result<AudienceGrants> {
        let rows = sqlx::query("SELECT audience, axis, selector FROM audience_grants")
            .fetch_all(pool)
            .await
            .context("querying audience_grants")?;
        let mut triples = Vec::with_capacity(rows.len());
        for row in rows {
            let audience: String = row.try_get("audience").context("reading audience")?;
            let axis: String = row.try_get("axis").context("reading axis")?;
            let selector: String = row.try_get("selector").context("reading selector")?;
            let axis = match axis.as_str() {
                "read" => GrantAxis::Read,
                "mint" => GrantAxis::Mint,
                other => {
                    return Err(anyhow!(
                        "audience_grants row for {audience:?}/{selector:?} has unrecognized \
                         axis {other:?}"
                    ));
                }
            };
            triples.push((audience, axis, selector));
        }
        AudienceGrants::from_rows(triples)
    }

    fn count_refresh_error() {
        micromegas_tracing::imetric!("audience_grant_refresh_error_count", "count", 1_u64);
    }
}

/// The whole-table snapshot cache described in the module doc comment.
pub type DbAudienceGrantsSource = SnapshotSource<AudienceGrantsLoader>;
