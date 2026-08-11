//! axum-utils : observability middleware

// crate-specific lint exceptions:
#![allow(clippy::missing_errors_doc)]

use anyhow::Result;
use async_stream::stream;
use axum::response::Response;
use axum::{extract::Request, middleware::Next};
use micromegas_analytics::response_writer::ResponseWriter;
use micromegas_tracing::prelude::*;
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;

use super::http_utils::get_client_ip;

/// Shared body for `observability_middleware` and `auth_observability_middleware`. Logs only
/// the request path (never the query string) when `log_query_string` is `false` -- used by
/// `auth_observability_middleware` because `/auth/callback`'s query carries the OAuth
/// authorization code and the PKCE verifier (embedded in the signed `state` param) and must
/// never be written to the telemetry log. The logged field is always named `uri=`, not `path=`
/// -- even when the value is path-only -- so queries that grep `uri=` to reconstruct HTTP traffic
/// across the whole `log_entries` stream still match `/auth/*` lines. `client_ip` reuses the
/// shared `get_client_ip` (rightmost `X-Forwarded-For` entry, then `X-Real-IP`, then the socket
/// address), same as every other route in this codebase.
async fn observability_middleware_impl(
    request: Request,
    next: Next,
    log_query_string: bool,
) -> Response {
    let (parts, body) = request.into_parts();
    let uri = if log_query_string {
        parts.uri.to_string()
    } else {
        parts.uri.path().to_string()
    };
    let client_ip = get_client_ip(&parts.headers, &parts.extensions);
    info!(
        "request method={} uri={uri} client_ip={client_ip}",
        parts.method
    );
    let begin_ticks = now();
    let response = next.run(Request::from_parts(parts, body)).await;
    let end_ticks = now();
    let duration = end_ticks - begin_ticks;
    imetric!("request_duration", "ticks", duration as u64);
    info!(
        "response status={} uri={uri} client_ip={client_ip}",
        response.status()
    );
    response
}

/// Logs http requests, their duration and status code, including the query string.
pub async fn observability_middleware(request: Request, next: Next) -> Response {
    observability_middleware_impl(request, next, true).await
}

/// Like `observability_middleware`, but never logs the query string -- see
/// `observability_middleware_impl`'s doc comment for why.
pub async fn auth_observability_middleware(request: Request, next: Next) -> Response {
    observability_middleware_impl(request, next, false).await
}

/// Makes a streaming body from a Tokio MPSC receiver.
pub fn make_body_from_channel_receiver(mut rx: Receiver<bytes::Bytes>) -> axum::body::Body {
    let read_stream = stream! {
        while let Some(value) = rx.recv().await{
                yield Result::<bytes::Bytes>::Ok(value);
        }
    };
    axum::body::Body::from_stream(read_stream)
}

/// Streams a response by executing a callback that writes to a `ResponseWriter`.
///
/// This function creates a channel and a `ResponseWriter` that writes to this channel.
/// The `callback` is then executed in a separate Tokio task, allowing it to stream data
/// back to the client as it becomes available.
pub fn stream_request<F, Fut>(callback: F) -> Response
where
    F: FnOnce(Arc<ResponseWriter>) -> Fut + 'static + Send,
    Fut: std::future::Future<Output = Result<()>> + Send,
{
    let (tx, rx) = tokio::sync::mpsc::channel(10);
    let writer = Arc::new(ResponseWriter::new(Some(tx)));
    let response_body = make_body_from_channel_receiver(rx);
    spawn_with_context(async move {
        let service_call = callback(writer.clone());
        if let Err(e) = service_call.await {
            if writer.is_closed() {
                info!("Error happened, but connection is closed: {e:?}");
            } else {
                // the connection is live, this looks like a real error
                error!("{e:?}");
                if let Err(e) = writer.write_string(format!("{e:?}")).await {
                    //error writing can happen, probably not a big deal
                    info!("{e:?}");
                }
            }
        }
    });

    Response::builder().status(200).body(response_body).unwrap()
}
