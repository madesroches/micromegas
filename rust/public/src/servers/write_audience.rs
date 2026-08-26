//! Resolving the authenticated write audience at the HTTP edge (AbAC Stage 5, #1373, §5; #1519).
//!
//! `rust/public` is the only crate that sees both `AuthContext` (`micromegas-auth`) and the
//! ingestion service (`micromegas-ingestion`), so this is where an `Option<&Extension<AuthContext>>`
//! -- the request extension every ingestion route now carries -- turns into a
//! [`micromegas_ingestion::write_audience::WriteAudience`]. `WriteAudience` is single-state
//! (#1519): a credential carrying no bound audience, or one that fails `WriteAudience::new`
//! validation, resolves to the deployment default here rather than to a distinct unstamped
//! state.

use axum::Extension;
use micromegas_auth::types::AuthContext;
use micromegas_ingestion::write_audience::WriteAudience;
use micromegas_tracing::prelude::*;

/// Resolves the write audience for one request: a credential carrying a bound audience gets
/// stamped with it (or, if that label fails [`WriteAudience::new`] validation, with
/// `default_audience` instead, `warn!`-logged); one without a bound audience -- or with no auth
/// provider at all, `ctx: None` -- resolves straight to `default_audience`. The result is always
/// a real audience: there is no unstamped state left to resolve to.
pub fn resolve_write_audience(
    ctx: Option<&Extension<AuthContext>>,
    default_audience: &WriteAudience,
) -> WriteAudience {
    let Some(bound) = ctx.and_then(|Extension(c)| c.bound_audience.as_deref()) else {
        return default_audience.clone();
    };
    match WriteAudience::new(bound) {
        Ok(w) => w,
        Err(e) => {
            warn!(
                "bound_audience failed WriteAudience validation, using the deployment default: {e:#}"
            );
            default_audience.clone()
        }
    }
}
