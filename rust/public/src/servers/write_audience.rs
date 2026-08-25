//! Resolving the authenticated write audience at the HTTP edge (AbAC Stage 5, #1373, §5).
//!
//! `rust/public` is the only crate that sees both `AuthContext` (`micromegas-auth`) and the
//! ingestion service (`micromegas-ingestion`), so this is where an `Option<&Extension<AuthContext>>`
//! -- the request extension every ingestion route now carries -- turns into a
//! [`micromegas_ingestion::write_audience::WriteAudience`].

use axum::Extension;
use micromegas_auth::types::AuthContext;
use micromegas_ingestion::write_audience::WriteAudience;
use micromegas_tracing::prelude::*;

/// Resolves the write audience for one request: a credential carrying a bound audience gets
/// stamped with it, and one without (or with no auth provider at all, `ctx: None`) stays
/// unstamped.
pub fn resolve_write_audience(ctx: Option<&Extension<AuthContext>>) -> WriteAudience {
    let audience = ctx.and_then(|Extension(c)| c.bound_audience.as_deref());
    match WriteAudience::new(audience) {
        Ok(w) => w,
        Err(e) => {
            warn!("bound_audience failed WriteAudience validation, ignoring: {e:#}");
            WriteAudience::none()
        }
    }
}
