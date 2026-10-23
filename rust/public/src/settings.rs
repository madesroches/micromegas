//! DB-backed deployment settings.
//!
//! The `settings` table (data-lake migration v12) is a generic key/value store. [`SETTINGS`] is
//! the registry of keys a service understands: it drives the admin page and holds the single
//! validation rule shared by the write path and the loader. A key with no row is "unset", and the
//! consuming service falls back to the env var, then to the built-in default.
//!
//! Each service caches the whole table through [`SnapshotSource`], the same cache the
//! audience-grant and group stores use, and reads typed values through [`SettingsSource`].

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use micromegas_analytics::delete::{MAX_RETENTION_DAYS, PayloadRetention, RetentionHorizon};
use micromegas_analytics::lakehouse::read_scope::IsolationConfig;
use micromegas_auth::db_audience_grants::DbAudienceGrantsConfig;
use micromegas_auth::db_snapshot::{SnapshotLoader, SnapshotSource};
use micromegas_tracing::prelude::*;
use sqlx::{PgPool, Row};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

pub const PUBLIC_VIEW_SETS: &str = "public_view_sets";
pub const RETENTION_DAYS: &str = "retention_days";
pub const RETENTION_BY_STREAM_TAG: &str = "retention_by_stream_tag";
pub const SELF_SERVICE_MINT: &str = "self_service_mint";
pub const SELF_SERVICE_MAX_CLAIMS_PER_CALLER: &str = "self_service_max_claims_per_caller";
pub const SELF_SERVICE_MAX_KEYS_PER_CALLER: &str = "self_service_max_keys_per_caller";
pub const SELF_SERVICE_MAX_GRANTS_PER_CALLER: &str = "self_service_max_grants_per_caller";

const DEFAULT_RETENTION_DAYS: u32 = 90;
const DEFAULT_MAX_CLAIMS_PER_CALLER: i64 = 25;
const DEFAULT_MAX_KEYS_PER_CALLER: i64 = 100;
const DEFAULT_MAX_GRANTS_PER_CALLER: i64 = 50;

/// The JSON shape a setting's value must have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    Bool,
    /// JSON integer in `0..=i64::MAX`.
    Count,
    /// JSON array of non-empty, trimmed, unique strings with no `[`, `]` or `"`.
    NameList,
    /// JSON integer in `1..=MAX_RETENTION_DAYS`.
    Days,
    /// JSON object mapping a stream tag to a `Days` integer or the string `"forever"`.
    TagHorizons,
}

impl SettingKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SettingKind::Bool => "bool",
            SettingKind::Count => "count",
            SettingKind::NameList => "name_list",
            SettingKind::Days => "days",
            SettingKind::TagHorizons => "tag_horizons",
        }
    }
}

/// One registered setting.
#[derive(Debug)]
pub struct SettingSpec {
    pub key: &'static str,
    /// `None` for a setting with no env fallback.
    pub env_var: Option<&'static str>,
    pub kind: SettingKind,
    /// JSON text of the built-in default, shown in the admin page.
    pub default: &'static str,
    pub description: &'static str,
}

pub const SETTINGS: &[SettingSpec] = &[
    SettingSpec {
        key: PUBLIC_VIEW_SETS,
        env_var: Some("MICROMEGAS_PUBLIC_VIEW_SETS"),
        kind: SettingKind::NameList,
        default: "[]",
        description: "View sets exempt from the row-level audience filter: every caller sees all of their rows.",
    },
    SettingSpec {
        key: RETENTION_DAYS,
        env_var: Some("MICROMEGAS_RETENTION_DAYS"),
        kind: SettingKind::Days,
        default: "90",
        description: "Default number of days raw payloads (blocks) and lakehouse partitions are kept.",
    },
    SettingSpec {
        key: RETENTION_BY_STREAM_TAG,
        env_var: None,
        kind: SettingKind::TagHorizons,
        default: "{}",
        description: "Per stream tag override of how long raw payloads are kept: a number of days or \"forever\". A stream with several overridden tags keeps the longest.",
    },
    SettingSpec {
        key: SELF_SERVICE_MINT,
        env_var: Some("MICROMEGAS_SELF_SERVICE_MINT"),
        kind: SettingKind::Bool,
        default: "false",
        description: "Let non-admin callers mint their own ingestion keys and manage audience grants.",
    },
    SettingSpec {
        key: SELF_SERVICE_MAX_CLAIMS_PER_CALLER,
        env_var: Some("MICROMEGAS_SELF_SERVICE_MAX_CLAIMS_PER_CALLER"),
        kind: SettingKind::Count,
        default: "25",
        description: "Maximum number of distinct audiences one non-admin caller may claim.",
    },
    SettingSpec {
        key: SELF_SERVICE_MAX_KEYS_PER_CALLER,
        env_var: Some("MICROMEGAS_SELF_SERVICE_MAX_KEYS_PER_CALLER"),
        kind: SettingKind::Count,
        default: "100",
        description: "Maximum number of ingestion keys a non-admin caller may hold.",
    },
    SettingSpec {
        key: SELF_SERVICE_MAX_GRANTS_PER_CALLER,
        env_var: Some("MICROMEGAS_SELF_SERVICE_MAX_GRANTS_PER_CALLER"),
        kind: SettingKind::Count,
        default: "50",
        description: "Maximum number of audience grants one non-admin caller may have created, excluding their own user:<email> rows.",
    },
];

/// Looks a key up in [`SETTINGS`].
pub fn find_spec(key: &str) -> Option<&'static SettingSpec> {
    SETTINGS.iter().find(|s| s.key == key)
}

impl SettingSpec {
    /// The single validation rule used by the PUT handler and the loader.
    pub fn validate(&self, value: &serde_json::Value) -> Result<()> {
        match self.kind {
            SettingKind::Bool => {
                if !value.is_boolean() {
                    bail!("{}: expected a JSON boolean", self.key);
                }
            }
            SettingKind::Count => {
                if value.as_i64().is_none_or(|n| n < 0) {
                    bail!(
                        "{}: expected a JSON integer between 0 and {}",
                        self.key,
                        i64::MAX
                    );
                }
            }
            SettingKind::NameList => {
                name_list(self.key, value)?;
            }
            SettingKind::Days => {
                days(self.key, value)?;
            }
            SettingKind::TagHorizons => {
                parse_tag_horizons(self.key, value)?;
            }
        }
        Ok(())
    }
}

fn days(key: &str, value: &serde_json::Value) -> Result<u32> {
    value
        .as_u64()
        .filter(|n| (1..=u64::from(MAX_RETENTION_DAYS)).contains(n))
        .map(|n| n as u32)
        .ok_or_else(|| anyhow!("{key}: expected a JSON integer between 1 and {MAX_RETENTION_DAYS}"))
}

/// Parses `{tag: days | "forever"}`.
pub fn parse_tag_horizons(
    key: &str,
    value: &serde_json::Value,
) -> Result<BTreeMap<String, RetentionHorizon>> {
    let map = value
        .as_object()
        .ok_or_else(|| anyhow!("{key}: expected a JSON object of tag to days or \"forever\""))?;
    let mut out = BTreeMap::new();
    for (tag, horizon) in map {
        if tag.is_empty() {
            bail!("{key}: empty tag");
        }
        if tag.trim() != tag {
            bail!("{key}: tag {tag:?} has leading or trailing whitespace");
        }
        let horizon = if horizon.as_str() == Some("forever") {
            RetentionHorizon::Forever
        } else {
            RetentionHorizon::Days(days(key, horizon).map_err(|_| {
                anyhow!(
                    "{key}: horizon for tag {tag:?} must be an integer between 1 and {MAX_RETENTION_DAYS}, or \"forever\""
                )
            })?)
        };
        out.insert(tag.clone(), horizon);
    }
    Ok(out)
}

fn name_list(key: &str, value: &serde_json::Value) -> Result<Vec<String>> {
    let items = value
        .as_array()
        .ok_or_else(|| anyhow!("{key}: expected a JSON array of strings"))?;
    let mut names: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        let name = item
            .as_str()
            .ok_or_else(|| anyhow!("{key}: entry {item} is not a string"))?;
        if name.is_empty() {
            bail!("{key}: empty entry");
        }
        if name.trim() != name {
            bail!("{key}: entry {name:?} has leading or trailing whitespace");
        }
        if name.contains(['[', ']', '"']) {
            bail!("{key}: entry {name:?} contains '[', ']', or '\"'");
        }
        if names.iter().any(|n| n == name) {
            bail!("{key}: duplicate entry {name:?}");
        }
        names.push(name.to_string());
    }
    Ok(names)
}

/// Self-service knobs resolved to concrete values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelfServiceSettings {
    pub mint_enabled: bool,
    pub max_claims_per_caller: i64,
    pub max_keys_per_caller: i64,
    pub max_grants_per_caller: i64,
}

impl Default for SelfServiceSettings {
    fn default() -> Self {
        Self {
            mint_enabled: false,
            max_claims_per_caller: DEFAULT_MAX_CLAIMS_PER_CALLER,
            max_keys_per_caller: DEFAULT_MAX_KEYS_PER_CALLER,
            max_grants_per_caller: DEFAULT_MAX_GRANTS_PER_CALLER,
        }
    }
}

/// The DB values, already parsed into typed fields. `None` means no row (or an unregistered key).
#[derive(Debug, Default)]
pub struct DbOverrides {
    pub isolation: Option<Arc<IsolationConfig>>,
    pub self_service_mint: Option<bool>,
    pub max_claims_per_caller: Option<i64>,
    pub max_keys_per_caller: Option<i64>,
    pub max_grants_per_caller: Option<i64>,
    pub retention_days: Option<u32>,
    pub retention_by_stream_tag: Option<BTreeMap<String, RetentionHorizon>>,
}

impl DbOverrides {
    /// Builds the typed set from `(key, value)` rows. Keys missing from [`SETTINGS`] are ignored,
    /// because during a rolling upgrade a newer replica may have written keys an older one does
    /// not know. A known key that fails validation fails the whole parse rather than falling back
    /// to env for that key, which could silently widen exposure.
    pub fn from_rows<I>(rows: I) -> Result<Self>
    where
        I: IntoIterator<Item = (String, serde_json::Value)>,
    {
        let mut out = Self::default();
        for (key, value) in rows {
            let Some(spec) = find_spec(&key) else {
                continue;
            };
            spec.validate(&value)
                .with_context(|| format!("invalid stored value for settings.{key}"))?;
            match spec.key {
                PUBLIC_VIEW_SETS => {
                    out.isolation = Some(Arc::new(IsolationConfig {
                        public_view_sets: name_list(spec.key, &value)?,
                    }));
                }
                SELF_SERVICE_MINT => out.self_service_mint = value.as_bool(),
                SELF_SERVICE_MAX_CLAIMS_PER_CALLER => out.max_claims_per_caller = value.as_i64(),
                SELF_SERVICE_MAX_KEYS_PER_CALLER => out.max_keys_per_caller = value.as_i64(),
                SELF_SERVICE_MAX_GRANTS_PER_CALLER => out.max_grants_per_caller = value.as_i64(),
                RETENTION_DAYS => out.retention_days = Some(days(spec.key, &value)?),
                RETENTION_BY_STREAM_TAG => {
                    out.retention_by_stream_tag = Some(parse_tag_horizons(spec.key, &value)?);
                }
                other => bail!("settings key {other:?} is registered but not handled"),
            }
        }
        Ok(out)
    }

    /// Keys that have a stored value.
    pub fn stored_keys(&self) -> Vec<&'static str> {
        let mut keys = Vec::new();
        if self.isolation.is_some() {
            keys.push(PUBLIC_VIEW_SETS);
        }
        if self.self_service_mint.is_some() {
            keys.push(SELF_SERVICE_MINT);
        }
        if self.max_claims_per_caller.is_some() {
            keys.push(SELF_SERVICE_MAX_CLAIMS_PER_CALLER);
        }
        if self.max_keys_per_caller.is_some() {
            keys.push(SELF_SERVICE_MAX_KEYS_PER_CALLER);
        }
        if self.max_grants_per_caller.is_some() {
            keys.push(SELF_SERVICE_MAX_GRANTS_PER_CALLER);
        }
        if self.retention_days.is_some() {
            keys.push(RETENTION_DAYS);
        }
        if self.retention_by_stream_tag.is_some() {
            keys.push(RETENTION_BY_STREAM_TAG);
        }
        keys
    }
}

/// [`SnapshotLoader`] for the `settings` table.
#[derive(Debug)]
pub struct SettingsLoader;

#[async_trait]
impl SnapshotLoader for SettingsLoader {
    type Snapshot = DbOverrides;
    const NAME: &'static str = "settings store";

    async fn fetch(pool: &PgPool) -> Result<DbOverrides> {
        // Read as text and parsed here so the crate does not need sqlx's `json` feature.
        let rows = sqlx::query("SELECT key, value::text AS value FROM settings")
            .fetch_all(pool)
            .await
            .context("querying settings")?;
        let mut pairs = Vec::with_capacity(rows.len());
        for row in rows {
            let key: String = row.try_get("key").context("reading key")?;
            let raw: String = row.try_get("value").context("reading value")?;
            let value = serde_json::from_str(&raw)
                .with_context(|| format!("parsing stored value for settings.{key}"))?;
            pairs.push((key, value));
        }
        DbOverrides::from_rows(pairs)
    }

    fn count_refresh_error() {
        micromegas_tracing::imetric!("settings_refresh_error_count", "count", 1_u64);
    }
}

/// Parses a boolean env knob the `v == "true" || v == "1"` way.
pub fn resolve_bool_env(var: &str, default: bool) -> bool {
    std::env::var(var)
        .map(|v| v == "true" || v == "1")
        .unwrap_or(default)
}

/// Parses an unsigned integer env knob, warn-and-default on an unparseable value. Parsing
/// unsigned makes a negative value a parse failure, and `0` a valid "none permitted".
pub fn resolve_u64_env(var: &str, default: u64) -> u64 {
    match std::env::var(var) {
        Ok(raw) => raw.parse::<u64>().unwrap_or_else(|_| {
            warn!("Invalid {var} value '{raw}', using default {default}");
            default
        }),
        Err(_) => default,
    }
}

/// Same as [`resolve_u64_env`], for a knob stored as `i64` downstream: a value that overflows
/// `i64` warns and defaults rather than wrapping negative.
pub fn resolve_i64_env(var: &str, default: i64) -> i64 {
    let value = resolve_u64_env(var, default as u64);
    i64::try_from(value).unwrap_or_else(|_| {
        warn!("{var} value {value} exceeds i64::MAX, using default {default}");
        default
    })
}

/// The env-and-default half of the resolution order, resolved once per process at startup.
#[derive(Debug, Clone)]
pub struct EnvFallbacks {
    pub isolation: Arc<IsolationConfig>,
    pub self_service: SelfServiceSettings,
    /// Fallback for `retention_days`; the maintenance services overwrite it with the
    /// `--retention-days` flag, which already folds in the env var.
    pub retention_days: u32,
}

impl EnvFallbacks {
    /// Fails fast on a malformed `MICROMEGAS_PUBLIC_VIEW_SETS`.
    pub fn from_env() -> Result<Self> {
        let isolation = Arc::new(IsolationConfig::from_env()?);
        let spec = |key| {
            find_spec(key)
                .expect("registered key")
                .env_var
                .expect("setting has an env var")
        };
        let self_service = SelfServiceSettings {
            mint_enabled: resolve_bool_env(spec(SELF_SERVICE_MINT), false),
            max_claims_per_caller: resolve_i64_env(
                spec(SELF_SERVICE_MAX_CLAIMS_PER_CALLER),
                DEFAULT_MAX_CLAIMS_PER_CALLER,
            ),
            max_keys_per_caller: resolve_i64_env(
                spec(SELF_SERVICE_MAX_KEYS_PER_CALLER),
                DEFAULT_MAX_KEYS_PER_CALLER,
            ),
            max_grants_per_caller: resolve_i64_env(
                spec(SELF_SERVICE_MAX_GRANTS_PER_CALLER),
                DEFAULT_MAX_GRANTS_PER_CALLER,
            ),
        };
        Ok(Self {
            isolation,
            self_service,
            retention_days: DEFAULT_RETENTION_DAYS,
        })
    }
}

/// `MICROMEGAS_AUTH_CACHE_TTL_SECONDS`, which governs every snapshot cache in the process.
pub fn settings_cache_ttl() -> Duration {
    Duration::from_secs(DbAudienceGrantsConfig::from_env_with_prefix("").cache_ttl_secs)
}

enum Backing {
    Db(SnapshotSource<SettingsLoader>),
    Fixed(Arc<DbOverrides>),
}

/// Typed access to the settings, resolving DB row, then env var, then built-in default.
pub struct SettingsSource {
    db: Backing,
    env: EnvFallbacks,
}

impl std::fmt::Debug for SettingsSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsSource").finish_non_exhaustive()
    }
}

impl SettingsSource {
    /// `env` is already resolved by the caller; `pool` should be small and bounded (e.g. from
    /// `dedicated_key_store_pool`, or the service's existing key-store pool).
    pub fn new(env: EnvFallbacks, pool: PgPool, ttl: Duration) -> Self {
        Self {
            db: Backing::Db(SnapshotSource::new(pool, ttl)),
            env,
        }
    }

    /// For a process with no telemetry-DB pool configured.
    pub fn env_only(env: EnvFallbacks) -> Self {
        Self::with_overrides(env, DbOverrides::default())
    }

    /// An explicit isolation config that ignores both env and DB, for `with_isolation_config`
    /// and tests. The self-service half is the built-in defaults.
    pub fn pinned(isolation: Arc<IsolationConfig>) -> Self {
        Self::env_only(EnvFallbacks {
            isolation,
            self_service: SelfServiceSettings::default(),
            retention_days: DEFAULT_RETENTION_DAYS,
        })
    }

    /// A fixed set of DB values, never reading process env or a database.
    pub fn with_overrides(env: EnvFallbacks, db: DbOverrides) -> Self {
        Self {
            db: Backing::Fixed(Arc::new(db)),
            env,
        }
    }

    /// The env-resolved isolation config, with no DB access. Used where the value has no effect
    /// (auth disabled makes every scope `ReadScope::All`).
    pub fn env_isolation_config(&self) -> Arc<IsolationConfig> {
        self.env.isolation.clone()
    }

    async fn overrides(&self) -> Result<Arc<DbOverrides>> {
        match &self.db {
            Backing::Db(source) => source.current().await,
            Backing::Fixed(fixed) => Ok(fixed.clone()),
        }
    }

    pub async fn isolation_config(&self) -> Result<Arc<IsolationConfig>> {
        let overrides = self.overrides().await?;
        Ok(overrides
            .isolation
            .clone()
            .unwrap_or_else(|| self.env.isolation.clone()))
    }

    pub async fn self_service(&self) -> Result<SelfServiceSettings> {
        let o = self.overrides().await?;
        let e = &self.env.self_service;
        Ok(SelfServiceSettings {
            mint_enabled: o.self_service_mint.unwrap_or(e.mint_enabled),
            max_claims_per_caller: o.max_claims_per_caller.unwrap_or(e.max_claims_per_caller),
            max_keys_per_caller: o.max_keys_per_caller.unwrap_or(e.max_keys_per_caller),
            max_grants_per_caller: o.max_grants_per_caller.unwrap_or(e.max_grants_per_caller),
        })
    }

    /// The payload retention policy. Errors when the settings can't be loaded, so the caller
    /// never falls back to the default horizon and deletes what an override was keeping.
    pub async fn payload_retention(&self) -> Result<PayloadRetention> {
        let o = self.overrides().await?;
        Ok(PayloadRetention {
            default_days: o.retention_days.unwrap_or(self.env.retention_days),
            by_stream_tag: o.retention_by_stream_tag.clone().unwrap_or_default(),
        })
    }

    /// Best-effort startup diagnostic: warns about each env var that is set but ignored because
    /// the setting has a stored value.
    pub async fn warn_on_shadowed_env(&self) {
        match self.overrides().await {
            Ok(overrides) => {
                for spec in shadowed(&overrides.stored_keys(), |var| std::env::var(var).is_ok()) {
                    let env_var = spec.env_var.expect("shadowed specs have an env var");
                    warn!(
                        "{env_var} is set but ignored: settings.{} is stored",
                        spec.key
                    );
                }
            }
            Err(e) => {
                warn!(
                    "could not check for env vars shadowed by stored settings \
                     (best-effort startup diagnostic, not fatal): {e:#}"
                );
            }
        }
    }
}

/// The registered settings that have a stored row and whose env var is also set.
pub fn shadowed(
    stored_keys: &[&str],
    env_is_set: impl Fn(&str) -> bool,
) -> Vec<&'static SettingSpec> {
    SETTINGS
        .iter()
        .filter(|s| stored_keys.contains(&s.key) && s.env_var.is_some_and(&env_is_set))
        .collect()
}
