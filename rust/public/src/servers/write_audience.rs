//! Resolving the authenticated write audience at the HTTP edge (AbAC Stage 5, #1373, §5).
//!
//! `rust/public` is the only crate that sees both `AuthContext` (`micromegas-auth`) and the
//! ingestion service (`micromegas-ingestion`), so this is where an `Option<&Extension<AuthContext>>`
//! -- the request extension every ingestion route now carries -- turns into a
//! [`micromegas_ingestion::write_audience::WriteAudience`]. Every process gets an audience,
//! always (#1482 §0): a credential carrying a bound audience gets stamped with it, and one
//! without (or with no auth provider at all, `ctx: None`) gets the deployment's
//! `MICROMEGAS_DEFAULT_AUDIENCE`.

use axum::Extension;
use micromegas_auth::types::AuthContext;
use micromegas_ingestion::write_audience::WriteAudience;

/// Resolves the write audience for one request: a credential carrying a bound audience gets
/// stamped with it, and one without (or with no auth provider at all, `ctx: None`) gets
/// `default`.
///
/// A `bound_audience` that fails [`WriteAudience::new`]'s charset check is `Err`, not silently
/// degraded to `default`: with no unstamped state left, the only degrade available would move a
/// restricted key's writes into the default (often `public`) audience -- fail-open on the one
/// boundary this design exists to hold. The case is near-unreachable in practice
/// (`ingestion_api_keys.audience` is `CHECK`-constrained, and the other `bound_audience`
/// producers hard-code `None`), so rejecting it costs nothing.
pub fn resolve_write_audience(
    ctx: Option<&Extension<AuthContext>>,
    default: &WriteAudience,
) -> anyhow::Result<WriteAudience> {
    match ctx.and_then(|Extension(c)| c.bound_audience.as_deref()) {
        Some(bound) => WriteAudience::new(bound),
        None => Ok(default.clone()),
    }
}
