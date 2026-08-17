//! The authenticated write audience a request ingests under (AbAC Stage 5, #1373).
//!
//! `None` means the credential carries no audience -- data stays unstamped, exactly as before
//! this stage. See `WriteAudience` itself for the charset contract this validates against.

use std::sync::Arc;

/// The authenticated write audience a request ingests under (AbAC Stage 5, #1373).
/// `None` means the credential carries no audience -- data stays unstamped, exactly as
/// before this stage.
///
/// Per `rust/CLAUDE.md`'s Rust-API stance, every process-insert call site must state its
/// audience explicitly -- there is no `Default` impl, so a call site that has no audience to
/// pass must say so with [`WriteAudience::none`], and the compiler enumerates every site that
/// needs updating when this type's callers change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteAudience(Option<Arc<str>>);

impl WriteAudience {
    /// Builds a `WriteAudience` from an optional audience label.
    ///
    /// Rejects a malformed label rather than stamping it: `ingestion_api_keys.audience` is
    /// already `CHECK`-constrained to `[A-Za-z0-9_-]{1,255}`, so this is defence in depth
    /// against a future producer of `bound_audience` that doesn't go through that column. The
    /// charset check duplicates `micromegas_auth::policy::is_valid_audience` (`policy.rs`)
    /// rather than depending on `micromegas-auth` from `micromegas-ingestion` -- the same
    /// crate-boundary trade-off `read_scope.rs`'s `is_well_formed_audience` already makes for
    /// `micromegas-analytics`. Keep the three copies in step if the charset ever changes.
    pub fn new(audience: Option<&str>) -> anyhow::Result<Self> {
        match audience {
            None => Ok(Self(None)),
            Some(aud) => {
                if is_valid_audience(aud) {
                    Ok(Self(Some(Arc::from(aud))))
                } else {
                    Err(anyhow::anyhow!(
                        "invalid write audience {aud:?}: must match [A-Za-z0-9_-]{{1,255}}"
                    ))
                }
            }
        }
    }

    /// No audience at all -- the credential carries none, or none is available (e.g. no auth
    /// provider configured). Data stamped with this stays unstamped.
    pub fn none() -> Self {
        Self(None)
    }

    /// Returns the audience label, or `None` if this is an unstamped write.
    pub fn as_str(&self) -> Option<&str> {
        self.0.as_deref()
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
