//! The authenticated write audience a request ingests under.
//!
//! Always a real audience: a credential that carries none resolves to the deployment default at
//! the HTTP edge (`micromegas::servers::write_audience::resolve_write_audience`), the same
//! resolution every other surface that touches an audience already performs. See
//! `WriteAudience` itself for the charset contract this validates against.

use std::sync::Arc;

/// The authenticated write audience a request ingests under.
/// Always a real audience -- a credential that carries none resolves to the deployment default
/// at the HTTP edge rather than staying a distinct, unstamped third state.
///
/// Per `rust/CLAUDE.md`'s Rust-API stance, every process-insert call site must state its
/// audience explicitly -- there is no `Default` impl, so a call site with nothing of its own to
/// pass must pass the resolved deployment default, and the compiler enumerates every site that
/// needs updating when this type's callers change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteAudience(Arc<str>);

impl WriteAudience {
    /// Builds a `WriteAudience` from an audience label.
    ///
    /// Rejects a malformed label: `ingestion_api_keys.audience` is already `CHECK`-constrained
    /// to `[A-Za-z0-9_-]{1,255}`, so this is defence in depth against a future producer of
    /// `bound_audience` that doesn't go through that column. The HTTP-edge caller
    /// (`micromegas::servers::write_audience::resolve_write_audience`) does not treat this
    /// `Err` as a rejection -- it warns and degrades to the deployment default instead, since
    /// that caller has no `Result` to propagate the failure through. This is also the validating
    /// constructor for the deployment default itself, called once at startup. The charset check
    /// duplicates `micromegas_auth::policy::is_valid_audience` (`policy.rs`) rather than
    /// depending on `micromegas-auth` from `micromegas-ingestion` -- the same crate-boundary
    /// trade-off `read_scope.rs`'s `is_well_formed_audience` already makes for
    /// `micromegas-analytics`. Keep the three copies in step if the charset ever changes.
    pub fn new(audience: &str) -> anyhow::Result<Self> {
        if is_valid_audience(audience) {
            Ok(Self(Arc::from(audience)))
        } else {
            Err(anyhow::anyhow!(
                "invalid write audience {audience:?}: must match [A-Za-z0-9_-]{{1,255}}"
            ))
        }
    }

    /// The audience label. Never absent.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The OTLP id-derivation namespace `self` occupies, relative to `default`. `default` is the
    /// deployment's resolved default audience: it keeps the un-salted `NS_OTEL_PROCESS_V1`
    /// namespace, so a resolved-to-default caller and one explicitly bound to a label equal to
    /// the default derive the *same* ids. Every other audience gets its own salted namespace.
    ///
    /// Returns `None` (the un-salted default namespace) when `self == default`, `Some(self)`
    /// (a per-audience salted namespace) otherwise. Named and pulled out here, rather than
    /// inlined at each of the five `IdentityContext` construction sites, so the rule has exactly
    /// one home.
    pub fn id_namespace<'a>(&'a self, default: &WriteAudience) -> Option<&'a str> {
        (self.as_str() != default.as_str()).then_some(self.as_str())
    }
}

/// `true` if `aud` is a valid audience name: `[A-Za-z0-9_-]{1,255}`, checked in bytes. See
/// `WriteAudience::new`'s doc comment for why this is a local copy rather than a dependency on
/// `micromegas-auth`.
fn is_valid_audience(aud: &str) -> bool {
    !aud.is_empty()
        && aud.len() <= 255
        && aud
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
