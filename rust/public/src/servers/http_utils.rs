//! HTTP utilities for server implementations

/// Extracts the client IP address from HTTP headers and extensions, formatted as a string for
/// logging.
///
/// A thin wrapper over [`micromegas_auth::client_ip::resolve_client_ip`], which owns the actual
/// resolution logic (X-Forwarded-For / X-Real-IP / socket address priority, with the
/// `to_canonical()` normalization) -- moved into the `auth` crate so `IpAllowlist::allows` can
/// consult the same resolution for an authorization decision, not just for this function's
/// audit-logging callers. See that function's doc comment for the full priority order and
/// anti-spoofing rationale.
///
/// Returns "unknown" if no IP can be extracted.
pub fn get_client_ip(headers: &http::HeaderMap, extensions: &http::Extensions) -> String {
    micromegas_auth::client_ip::resolve_client_ip(headers, extensions)
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
