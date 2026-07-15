//! Shared body-limit / decompression layers for ingestion routes carrying large,
//! potentially-compressed bodies (OTLP and webhook). Factored out so every such
//! router applies identical limits instead of duplicating the constants.

use axum::Router;
use axum::extract::DefaultBodyLimit;
use tower_http::decompression::RequestDecompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;

/// 20 MiB matches the OTel Collector `confighttp.max_request_body_size` default —
/// anything an SDK is willing to send under the conventional Collector cap fits here too.
/// Applies to compressed wire bytes (the `RequestBodyLimitLayer` runs outside the
/// decompression layer).
pub(crate) const INGESTION_BODY_LIMIT_BYTES: usize = 20 * 1024 * 1024;

/// Cap on the decompressed body size the handler will materialize. Without this,
/// a malicious gzip payload up to `INGESTION_BODY_LIMIT_BYTES` could expand at gzip's
/// worst-case ratio (~1000×) and OOM the server. Sized at 15× the wire cap to
/// cover legitimate protobuf compression (commonly observed up to 10×) with
/// headroom, while still bounding the worst case to a survivable allocation.
pub(crate) const INGESTION_DECOMPRESSED_BODY_LIMIT_BYTES: usize = 300 * 1024 * 1024;

/// `Retry-After` value (in seconds) on retryable 503 responses. Conservative default —
/// tune based on observed recovery times.
pub(crate) const RETRY_AFTER_SECONDS: u32 = 30;

/// Applies the shared body-limit + gzip-decompression layers to `router`.
///
/// Layer order, outermost → innermost (request travels through them top to bottom):
///  1. `DefaultBodyLimit::max(300 MiB)` — caps the post-decompression bytes the
///     handler's `Bytes` extractor will materialize, defending against gzip-bomb
///     expansion that the wire-byte limit can't see.
///  2. `RequestBodyLimitLayer(20 MiB)` — caps the *compressed* wire bytes;
///     enforced before decompression, returning 413 on oversize.
///  3. `RequestDecompressionLayer` — gzip-decodes the body before the handler.
///  4. handler.
pub(crate) fn apply_ingestion_body_limits(router: Router) -> Router {
    router
        .layer(RequestDecompressionLayer::new().gzip(true))
        .layer(RequestBodyLimitLayer::new(INGESTION_BODY_LIMIT_BYTES))
        .layer(DefaultBodyLimit::max(
            INGESTION_DECOMPRESSED_BODY_LIMIT_BYTES,
        ))
}
