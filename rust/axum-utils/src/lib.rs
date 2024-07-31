//! axum-utils : observability middleware

// crate-specific lint exceptions:
#![allow(clippy::missing_errors_doc)]

use axum::response::Response;
use axum::{extract::Request, middleware::Next};
use micromegas_tracing::prelude::*;

pub async fn observability_middleware(request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    let uri = parts.uri.clone();
    info!("request method={} uri={uri}", parts.method);
    let begin_ticks = now();
    let response = next.run(Request::from_parts(parts, body)).await;
    let end_ticks = now();
    let duration = end_ticks - begin_ticks;
    imetric!("request_duration", "ticks", duration as u64);
    info!("response status={} uri={uri}", response.status());
    response
}
