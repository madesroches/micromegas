//! Ingestion key-management proxy for `analytics-web-srv` (#1411).
//!
//! `analytics-web-srv`'s `id_token` cookie is `http_only`, so browser JS has
//! no bearer token to attach to a direct `fetch()` against ingestion's
//! `/auth/api_keys*` routes — the ingestion-key admin page is therefore
//! necessarily a server-side proxy, forwarding on the operator's behalf under
//! this service's own privileged service credential. Per the design's admin
//! gating requirement, every wrapper below runs the [`AdminUser`] extractor
//! *before* [`forward`] is ever called — an unauthorized `analytics-web-srv`
//! caller never triggers a service-credential token fetch, let alone a call
//! to ingestion.
//!
//! **Known limitation, accepted:** ingestion has no way to see *which* admin
//! is behind a proxied call — every mint/list/revoke request it receives is
//! authenticated solely as this proxy's own service credential
//! (`MICROMEGAS_INGESTION_PROXY_OIDC_*`). So `ingestion_api_keys.created_by`/
//! `revoked_by` for every key minted or revoked through this web UI records
//! that service identity, not the human admin who clicked the button. An
//! earlier revision of this proxy forwarded the admin's own identity via a
//! header, but any scheme for ingestion to trust that header safely (i.e.
//! only from this proxy, not from an arbitrary admin caller) requires its own
//! distinct trust configuration — a second admin-adjacent allowlist to
//! provision and keep in sync — to close a narrow gap that only exists
//! *between* already-equally-privileged ingestion admins. Given the added
//! configuration surface and failure modes that requires, documenting the
//! limitation was judged the better trade for now; revisit if per-admin
//! attribution through the proxy becomes a real operational need.

use crate::auth::AdminUser;
use axum::body::{Body, Bytes};
use axum::extract::{Extension, Path, RawQuery};
use axum::http::{StatusCode, header::CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use http::Method;
use micromegas::telemetry_sink::oidc_client_credentials_decorator::OidcClientCredentialsDecorator;
use micromegas::telemetry_sink::request_decorator::RequestDecorator;
use micromegas::tracing::prelude::*;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Bound applied to every outbound admin-console call this proxy makes to
/// ingestion — both the token fetch inside `cfg.credentials.decorate(..)`
/// (whose own internal `reqwest::Client` has no timeout of its own; see
/// [`forward`]) and the forwarded request itself (via `cfg.client`'s
/// `.timeout(..)`). Without wrapping the former explicitly, a black-holed
/// `MICROMEGAS_INGESTION_PROXY_OIDC_TOKEN_ENDPOINT` would hang the admin's
/// browser request indefinitely, since `forward` awaits `decorate()` before
/// ever calling `cfg.client.execute(..)`.
const OUTBOUND_TIMEOUT: Duration = Duration::from_secs(10);

/// Config for reaching ingestion's admin API, plus the client that reaches
/// it. Held as `Extension<IngestionProxyState>` (`state.config: Option<Arc<..>>`,
/// `None` when unconfigured) so the proxy routes can be registered
/// unconditionally and return 503 per-request instead of being conditionally
/// merged.
pub struct IngestionProxyConfig {
    /// `MICROMEGAS_INGESTION_ADMIN_URL`, e.g. `"http://127.0.0.1:8081"`.
    pub base_url: String,
    /// Trait object, not the concrete `OidcClientCredentialsDecorator` —
    /// lets tests inject `TrivialRequestDecorator` and skip the token fetch
    /// entirely (the concrete decorator's fields are all private, with no
    /// way to pre-seed a cached token). Its `decorate()` call is itself
    /// wrapped in [`OUTBOUND_TIMEOUT`] by [`forward`] — the concrete
    /// `OidcClientCredentialsDecorator`'s internal token-fetch client has no
    /// timeout of its own.
    pub credentials: Arc<dyn RequestDecorator>,
    /// Single client, built once with an explicit [`OUTBOUND_TIMEOUT`]. This
    /// bounds only the forwarded request itself — the separate token fetch
    /// inside `credentials.decorate(..)` is bounded independently by
    /// [`forward`].
    pub client: reqwest::Client,
}

impl IngestionProxyConfig {
    /// Reads `MICROMEGAS_INGESTION_ADMIN_URL` and the four
    /// `MICROMEGAS_INGESTION_PROXY_OIDC_*` vars. Returns `None` (not an
    /// error) when the URL or the required credential trio (client id/
    /// secret/token endpoint) is unset — the caller is expected to log a
    /// `warn!` and keep starting, the same "unconfigured, not fatal" shape
    /// `maps::connect_maps_store` uses.
    ///
    /// Deliberately its own, distinctly-named credential — not
    /// `OidcClientCredentialsDecorator::from_env()`, which reads the
    /// self-telemetry `MICROMEGAS_OIDC_*` vars for a different identity (see
    /// the design doc's "Service credential" section).
    pub fn from_env() -> Option<Self> {
        let base_url = std::env::var("MICROMEGAS_INGESTION_ADMIN_URL").ok()?;
        let client_id = std::env::var("MICROMEGAS_INGESTION_PROXY_OIDC_CLIENT_ID").ok()?;
        let client_secret = std::env::var("MICROMEGAS_INGESTION_PROXY_OIDC_CLIENT_SECRET").ok()?;
        let token_endpoint =
            std::env::var("MICROMEGAS_INGESTION_PROXY_OIDC_TOKEN_ENDPOINT").ok()?;
        let audience = std::env::var("MICROMEGAS_INGESTION_PROXY_OIDC_AUDIENCE").ok();

        let client = reqwest::Client::builder()
            .timeout(OUTBOUND_TIMEOUT)
            .build()
            .expect("building the ingestion-proxy reqwest client");

        let credentials: Arc<dyn RequestDecorator> = Arc::new(OidcClientCredentialsDecorator::new(
            token_endpoint,
            client_id,
            client_secret,
            audience,
            180,
        ));

        Some(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            credentials,
            client,
        })
    }
}

#[derive(Clone)]
pub struct IngestionProxyState {
    pub config: Option<Arc<IngestionProxyConfig>>,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    code: String,
    message: String,
}

impl ErrorResponse {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

pub enum ProxyError {
    /// `state.config == None` — the proxy was never configured.
    NotConfigured,
    /// Building/sending the outbound request, or decorating it with a
    /// bearer token, failed.
    Request(String),
    /// Ingestion answered 404 with an **empty** body — axum's
    /// no-matching-route default, never `ApiKeyError`'s JSON shape. Distinct
    /// from a legitimate `ApiKeyError::NotFound` (non-empty JSON body),
    /// which is forwarded verbatim instead. Almost always means ingestion
    /// auth is disabled on the target service, so `/auth/api_keys*` was
    /// never mounted at all.
    IngestionRouteMissing(String),
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> Response {
        match self {
            ProxyError::NotConfigured => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse::new(
                    "NOT_CONFIGURED",
                    "ingestion key-management proxy not configured: set MICROMEGAS_INGESTION_ADMIN_URL and MICROMEGAS_INGESTION_PROXY_OIDC_*",
                )),
            )
                .into_response(),
            ProxyError::Request(msg) => {
                error!("ingestion_keys_proxy: {msg}");
                (
                    StatusCode::BAD_GATEWAY,
                    Json(ErrorResponse::new(
                        "PROXY_ERROR",
                        "failed to reach ingestion",
                    )),
                )
                    .into_response()
            }
            ProxyError::IngestionRouteMissing(msg) => {
                warn!("ingestion_keys_proxy: {msg}");
                (
                    StatusCode::BAD_GATEWAY,
                    Json(ErrorResponse::new("INGESTION_ROUTE_MISSING", msg)),
                )
                    .into_response()
            }
        }
    }
}

/// One HTTP round trip per call: never cached at process level beyond the
/// bearer token itself (inside `credentials`) — this is an admin-console
/// path, not a hot one.
///
/// Not an axum handler — a plain helper the `list`/`mint`/`revoke` wrappers
/// below call with already-extracted values; it performs no extraction
/// itself and takes no `user` parameter (forwarding never depends on *who*
/// the admin is, only that they are one).
async fn forward(
    state: IngestionProxyState,
    method: Method,
    path_suffix: &str,
    query: Option<&str>,
    body: Option<Bytes>,
) -> Result<Response, ProxyError> {
    let Some(cfg) = state.config else {
        return Err(ProxyError::NotConfigured);
    };

    // `reqwest::RequestBuilder::query` serializes via `serde_urlencoded`,
    // which only accepts maps/structs, not a raw `limit=10&offset=0` string
    // — append the query string to the URL directly instead.
    let url = format!(
        "{}{}{}",
        cfg.base_url,
        path_suffix,
        query.map(|q| format!("?{q}")).unwrap_or_default()
    );

    let mut req = cfg.client.request(method, url);
    if let Some(b) = body {
        // Required: ingestion's `mint_key`'s `Json<MintRequest>` extractor
        // 415s (`MissingJsonContentType`) on a request with no Content-Type
        // at all. Every body this proxy relays is JSON.
        req = req.body(b).header(CONTENT_TYPE, "application/json");
    }

    let mut built = req
        .build()
        .map_err(|e| ProxyError::Request(format!("building request: {e}")))?;
    // `decorate()` (for the real `OidcClientCredentialsDecorator`) fetches a
    // token using its own internal `reqwest::Client`, which has no timeout
    // of its own — bound it explicitly so a black-holed OIDC token endpoint
    // can't hang this admin request indefinitely ahead of the `cfg.client`
    // timeout below, which only covers the forwarded request itself.
    tokio::time::timeout(OUTBOUND_TIMEOUT, cfg.credentials.decorate(&mut built))
        .await
        .map_err(|_| {
            ProxyError::Request("decorating request: timed out fetching token".to_string())
        })?
        .map_err(|e| ProxyError::Request(format!("decorating request: {e}")))?;

    let resp = cfg
        .client
        .execute(built)
        .await
        .map_err(|e| ProxyError::Request(format!("calling ingestion: {e}")))?;

    let status = resp.status();
    let content_type = resp.headers().get(CONTENT_TYPE).cloned();
    let body_bytes = resp
        .bytes()
        .await
        .map_err(|e| ProxyError::Request(format!("reading ingestion response: {e}")))?;

    if status == StatusCode::NOT_FOUND && body_bytes.is_empty() {
        return Err(ProxyError::IngestionRouteMissing(format!(
            "ingestion returned 404 for {path_suffix} — is ingestion auth enabled on that service?"
        )));
    }

    // Forward ingestion's status + JSON body verbatim to the browser.
    let mut builder = Response::builder().status(status);
    if let Some(ct) = content_type {
        builder = builder.header(CONTENT_TYPE, ct);
    }
    builder
        .body(Body::from(body_bytes))
        .map_err(|e| ProxyError::Request(format!("building proxied response: {e}")))
}

/// `GET {base_path}/api/ingestion-api-keys?limit=&offset=&include_revoked=` —
/// forwards the incoming query string verbatim so the frontend can reuse the
/// same paging UI as the analytics-key page.
async fn list(
    Extension(state): Extension<IngestionProxyState>,
    AdminUser(_user): AdminUser,
    RawQuery(query): RawQuery,
) -> Result<Response, ProxyError> {
    forward(state, Method::GET, "/auth/api_keys", query.as_deref(), None).await
}

/// `POST {base_path}/api/ingestion-api-keys`.
///
/// `AdminUser` is `FromRequestParts`, so it runs *before* the `Bytes` body
/// extractor — a non-admin's body is never buffered.
async fn mint(
    Extension(state): Extension<IngestionProxyState>,
    AdminUser(_user): AdminUser,
    body: Bytes,
) -> Result<Response, ProxyError> {
    forward(state, Method::POST, "/auth/api_keys", None, Some(body)).await
}

/// `DELETE {base_path}/api/ingestion-api-keys/{key_id}`.
async fn revoke(
    Extension(state): Extension<IngestionProxyState>,
    AdminUser(_user): AdminUser,
    Path(key_id): Path<Uuid>,
) -> Result<Response, ProxyError> {
    forward(
        state,
        Method::DELETE,
        &format!("/auth/api_keys/{key_id}"),
        None,
        None,
    )
    .await
}

pub fn ingestion_keys_proxy_router(base_path: &str) -> Router {
    Router::new()
        .route(
            &format!("{base_path}/api/ingestion-api-keys"),
            get(list).post(mint),
        )
        .route(
            &format!("{base_path}/api/ingestion-api-keys/{{key_id}}"),
            delete(revoke),
        )
}
