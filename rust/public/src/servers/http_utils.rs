//! HTTP utilities for server implementations

/// Extracts the client IP address from HTTP headers and extensions.
///
/// This function checks headers in order of priority:
/// 1. X-Forwarded-For (leftmost IP is the original client when behind proxies)
/// 2. X-Real-IP (used by some proxies like nginx)
/// 3. Socket address from extensions (direct connection)
///
/// Returns "unknown" if no IP can be extracted.
pub fn get_client_ip(headers: &http::HeaderMap, extensions: &http::Extensions) -> String {
    // Check X-Forwarded-For header first (for load balancers/proxies)
    // The leftmost IP is the original client when behind proxies
    if let Some(forwarded_for) = headers.get("x-forwarded-for")
        && let Ok(value) = forwarded_for.to_str()
        && let Some(client_ip) = value.split(',').next()
    {
        return client_ip.trim().to_string();
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
