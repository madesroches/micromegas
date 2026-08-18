//! Resolving the authenticated write audience at the HTTP edge (AbAC Stage 5, #1373, §5).
//!
//! `rust/public` is the only crate that sees both `AuthContext` (`micromegas-auth`) and the
//! ingestion service (`micromegas-ingestion`), so this is where an `Option<&Extension<AuthContext>>`
//! -- the request extension every ingestion route now carries -- turns into a
//! [`micromegas_ingestion::write_audience::WriteAudience`].

use axum::Extension;
use micromegas_auth::env::resolve_prefixed_var;
use micromegas_auth::types::AuthContext;
use micromegas_ingestion::write_audience::WriteAudience;
use micromegas_tracing::prelude::*;

/// Per-service stamping config, resolved once at startup from `{prefix}_REQUIRE_WRITE_AUDIENCE`
/// (falling back to unprefixed `MICROMEGAS_REQUIRE_WRITE_AUDIENCE`). Off by default: a
/// deployment that has not migrated every producer onto an audience-bound DB ingestion key must
/// not have its env-keyring/OIDC producers rejected the moment this stage ships.
#[derive(Debug, Clone, Copy)]
pub struct StampingConfig {
    require_write_audience: bool,
}

impl StampingConfig {
    /// Resolves `{prefix}_REQUIRE_WRITE_AUDIENCE` (falling back to
    /// `MICROMEGAS_REQUIRE_WRITE_AUDIENCE`). Unset or empty -> `false` (the inert default).
    /// Any other value is parsed via [`str::parse::<bool>`] (`"true"`/`"false"`, case-sensitive)
    /// -- a malformed value is `Err`, not silently treated as `false`, matching every other
    /// fail-fast-on-typo knob in this rollout.
    pub fn from_env(prefix: &str) -> anyhow::Result<Self> {
        let var = resolve_prefixed_var(prefix, "REQUIRE_WRITE_AUDIENCE");
        let require_write_audience = match std::env::var(&var) {
            Ok(raw) if raw.trim().is_empty() => false,
            Ok(raw) => raw.trim().parse::<bool>().map_err(|_| {
                anyhow::anyhow!(
                    "{var}: {raw:?} is not a valid boolean -- use \"true\" or \"false\""
                )
            })?,
            Err(_) => false,
        };
        if require_write_audience {
            info!("{var}: true -- rejecting writes from a credential carrying no audience");
        }
        Ok(Self {
            require_write_audience,
        })
    }

    /// Builds a config directly, bypassing env resolution -- for tests.
    pub fn new(require_write_audience: bool) -> Self {
        Self {
            require_write_audience,
        }
    }
}

/// `resolve_write_audience` couldn't stamp: `Err` is a 403, never a silent unstamped write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteAudienceError;

impl std::fmt::Display for WriteAudienceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("write audience required")
    }
}

impl std::error::Error for WriteAudienceError {}

/// Resolves the write audience for one request.
///
/// | Credential | `require_write_audience` off (default) | on |
/// |---|---|---|
/// | DB ingestion key (`bound_audience: Some`) | stamp it | stamp it |
/// | Env-keyring key / OIDC (`None`) | unstamped | **403** |
/// | No auth provider (no extension, `ctx: None`) | unstamped | **403** |
///
/// `Err` is a 403 at every call site -- never a silent unstamped write when the operator asked
/// for enforcement.
pub fn resolve_write_audience(
    ctx: Option<&Extension<AuthContext>>,
    cfg: &StampingConfig,
) -> Result<WriteAudience, WriteAudienceError> {
    let audience = ctx.and_then(|Extension(c)| c.bound_audience.as_deref());
    match audience {
        Some(_) => WriteAudience::new(audience).map_err(|e| {
            warn!("bound_audience failed WriteAudience validation, rejecting: {e:#}");
            WriteAudienceError
        }),
        None if cfg.require_write_audience => {
            warn!(
                "rejecting write: no write audience on this credential and REQUIRE_WRITE_AUDIENCE is set"
            );
            Err(WriteAudienceError)
        }
        None => Ok(WriteAudience::none()),
    }
}
