//! HTTP utilities for server implementations

/// Extracts the client IP address from HTTP headers and extensions.
///
/// This function checks sources in order of priority:
/// 1. X-Forwarded-For (rightmost entry of the last header field line -- the address the nearest
///    trusted proxy observed)
/// 2. X-Real-IP (used by some proxies like nginx)
/// 3. Socket address from extensions (direct connection)
///
/// The *rightmost* `X-Forwarded-For` entry is used, not the leftmost, because the AWS ALB every
/// service is deployed behind *appends* the address it observed rather than overwriting the
/// header (`routing.http.xff_header_processing.mode = append`, the ALB default). Every entry to
/// the left of the last one is caller-supplied and therefore spoofable; the last entry is the
/// ALB's own observation and cannot be forged by the caller *for requests that actually traversed
/// the ALB* -- this holds even if the caller sends its own `X-Forwarded-For` as a *separate*
/// header field line, since `HeaderMap::get` returns only the first such line and would surface a
/// fully caller-chosen value; `get_all(...).iter().next_back()` is required to reach the ALB's
/// line. This is correct for exactly one trusted proxy hop -- putting a second trusted proxy in front of the ALB
/// would mean skipping one more entry from the right. For a request that reaches this service
/// *without* going through the ALB (local dev, an in-cluster peer, or any other direct connection),
/// there is no trusted proxy appending anything, so the rightmost entry is just as caller-chosen
/// as every other entry and this guarantee does not apply -- the socket-address fallback (branch
/// 3) is the only non-forgeable source in that case.
///
/// Returns "unknown" if no IP can be extracted.
pub fn get_client_ip(headers: &http::HeaderMap, extensions: &http::Extensions) -> String {
    // Check X-Forwarded-For header first (for load balancers/proxies).
    // `get_all(...).iter().next_back()` -- not `get(...)`, which returns only the *first* field
    // line -- picks the last field line, since the ALB appends its own observation as (or onto)
    // the last line; within that line, the rightmost comma-separated entry is what the ALB
    // itself observed. Everything else (earlier lines in full, and earlier entries within the
    // last line) is caller-supplied and spoofable. `next_back()` (not `.last()`, which would
    // needlessly walk the whole iterator) is equivalent here since `GetAll::iter()` is a
    // `DoubleEndedIterator`.
    if let Some(forwarded_for) = headers.get_all("x-forwarded-for").iter().next_back()
        && let Ok(value) = forwarded_for.to_str()
        && let Some(client_ip) = value.rsplit(',').map(str::trim).find(|s| !s.is_empty())
    {
        return client_ip.to_string();
    }

    // Check X-Real-IP header (used by some proxies like nginx)
    if let Some(real_ip) = headers.get("x-real-ip")
        && let Ok(value) = real_ip.to_str()
    {
        return value.to_string();
    }

    // Fall back to socket address from extensions
    // Axum provides ConnectInfo<SocketAddr>, Tonic provides SocketAddr directly
    if let Some(connect_info) = extensions.get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
    {
        return connect_info.0.ip().to_string();
    }

    if let Some(remote_addr) = extensions.get::<std::net::SocketAddr>() {
        return remote_addr.ip().to_string();
    }

    "unknown".to_string()
}
