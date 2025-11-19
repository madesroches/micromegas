use anyhow::{Context, Result};
use axum::{
    Extension, Json, Router,
    body::Body,
    extract::ConnectInfo,
    http::{Response, StatusCode},
    response::IntoResponse,
    routing::post,
};
use datafusion::arrow::{
    array::RecordBatch,
    json::{Writer, writer::JsonArray},
};
use http::{HeaderMap, Uri};
use micromegas_tracing::info;
use serde::Deserialize;
use std::net::SocketAddr;
use std::sync::Arc;
use thiserror::Error;
use tonic::transport::{Channel, ClientTlsConfig};

use crate::client::flightsql_client::Client;
use crate::servers::http_utils;

/// Configuration for forwarding HTTP headers to FlightSQL backend
#[derive(Debug, Clone, Deserialize)]
pub struct HeaderForwardingConfig {
    /// Exact header names to forward (case-insensitive)
    pub allowed_headers: Vec<String>,

    /// Header prefixes to forward (e.g., "X-Custom-")
    pub allowed_prefixes: Vec<String>,

    /// Headers to explicitly block (overrides allows)
    pub blocked_headers: Vec<String>,
}

impl Default for HeaderForwardingConfig {
    fn default() -> Self {
        Self {
            // Default safe headers to forward
            allowed_headers: vec![
                "Authorization".to_string(),
                "User-Agent".to_string(),
                "X-Client-Type".to_string(),
                "X-Correlation-ID".to_string(),
                "X-Request-ID".to_string(),
                "X-User-Email".to_string(),
                "X-User-ID".to_string(),
                "X-User-Name".to_string(),
            ],
            allowed_prefixes: vec![],
            blocked_headers: vec![
                "Cookie".to_string(),
                "Set-Cookie".to_string(),
                // SECURITY: Gateway always sets this from actual connection
                "X-Client-IP".to_string(),
            ],
        }
    }
}

impl HeaderForwardingConfig {
    /// Load configuration from environment variable or use defaults
    pub fn from_env() -> Result<Self> {
        if let Ok(config_json) = std::env::var("MICROMEGAS_GATEWAY_HEADERS") {
            serde_json::from_str(&config_json).context("Failed to parse MICROMEGAS_GATEWAY_HEADERS")
        } else {
            Ok(Self::default())
        }
    }

    /// Check if a header should be forwarded based on configuration
    pub fn should_forward(&self, header_name: &str) -> bool {
        let name_lower = header_name.to_lowercase();

        // Check blocked list first
        if self
            .blocked_headers
            .iter()
            .any(|h| h.to_lowercase() == name_lower)
        {
            return false;
        }

        // Check exact matches
        if self
            .allowed_headers
            .iter()
            .any(|h| h.to_lowercase() == name_lower)
        {
            return true;
        }

        // Check prefixes
        self.allowed_prefixes
            .iter()
            .any(|prefix| name_lower.starts_with(&prefix.to_lowercase()))
    }
}

#[derive(Error, Debug)]
pub enum GatewayError {
    #[error("Internal server error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response<Body> {
        let (status, message) = match &self {
            GatewayError::Internal(err) => {
                let msg = format!("{err:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, msg)
            }
        };
        (status, message).into_response()
    }
}

#[derive(Debug, Deserialize)]
pub struct QueryRequest {
    sql: String,
}

/// Build origin tracking metadata for FlightSQL queries
/// Augments the client type by appending "+gateway" to preserve the full client chain
///
/// This function only sets origin tracking headers that the gateway controls:
/// - x-client-type: augmented with "+gateway"
/// - x-request-id: generated if not present
/// - x-client-ip: extracted from actual connection (prevents spoofing)
///
/// User attribution headers (x-user-id, x-user-email) are forwarded from client
/// if present in allowed_headers config. FlightSQL validates these against the
/// Authorization token.
pub fn build_origin_metadata(
    headers: &HeaderMap,
    addr: &SocketAddr,
) -> tonic::metadata::MetadataMap {
    let mut metadata = tonic::metadata::MetadataMap::new();

    // 1. Client Type - augment existing or set to "unknown+gateway"
    let original_client_type = headers
        .get("x-client-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let augmented_client_type = format!("{original_client_type}+gateway");
    if let Ok(value) = augmented_client_type.parse() {
        metadata.insert("x-client-type", value);
    }

    // 2. Request ID - generate UUID if not present
    let request_id = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    if let Ok(value) = request_id.parse() {
        metadata.insert("x-request-id", value);
    }

    // 3. Client IP - ALWAYS extract from connection (never from client header)
    // SECURITY: Prevents IP spoofing in audit logs
    let mut extensions = http::Extensions::new();
    extensions.insert(axum::extract::ConnectInfo(*addr));
    let client_ip = http_utils::get_client_ip(headers, &extensions);
    if let Ok(value) = client_ip.parse() {
        metadata.insert("x-client-ip", value);
    }

    metadata
}

pub async fn handle_query(
    Extension(config): Extension<Arc<HeaderForwardingConfig>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<QueryRequest>,
) -> Result<String, GatewayError> {
    let flight_url = std::env::var("MICROMEGAS_FLIGHTSQL_URL")
        .with_context(|| "error reading MICROMEGAS_FLIGHTSQL_URL environment variable")?
        .parse::<Uri>()
        .with_context(|| "parsing flightsql url")?;
    let tls_config = ClientTlsConfig::new().with_native_roots();
    let channel = Channel::builder(flight_url)
        .tls_config(tls_config)
        .with_context(|| "tls_config")?
        .connect()
        .await
        .with_context(|| "connecting grpc channel")?;

    // Build origin tracking metadata
    let origin_metadata = build_origin_metadata(&headers, &addr);

    // Create client
    let mut client = Client::new(channel);

    // Set origin metadata as headers first
    // These are: x-client-type, x-request-id, x-client-ip
    let client_type_header = origin_metadata
        .get("x-client-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown+gateway");
    let request_id_header = origin_metadata
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");

    client
        .inner_mut()
        .set_header("x-client-type", client_type_header);
    client
        .inner_mut()
        .set_header("x-request-id", request_id_header);

    if let Some(client_ip) = origin_metadata.get("x-client-ip") {
        if let Ok(ip_str) = client_ip.to_str() {
            client.inner_mut().set_header("x-client-ip", ip_str);
        }
    }

    // Forward allowed headers from client
    for (name, value) in headers.iter() {
        let header_name = name.as_str();

        // Skip headers already set by origin metadata
        if header_name.eq_ignore_ascii_case("x-client-type")
            || header_name.eq_ignore_ascii_case("x-request-id")
            || header_name.eq_ignore_ascii_case("x-client-ip")
        {
            continue; // Origin metadata takes precedence
        }

        if config.should_forward(header_name) {
            if let Ok(value_str) = value.to_str() {
                client.inner_mut().set_header(header_name, value_str);
            }
        }
    }

    info!(
        "Gateway request: request_id={}, client_type={}, sql={}",
        request_id_header, client_type_header, &request.sql
    );

    let batches = client.query(request.sql, None).await?;
    if batches.is_empty() {
        return Ok("[]".to_string());
    }

    let mut buffer = Vec::new();
    let mut json_writer = Writer::<_, JsonArray>::new(&mut buffer);
    let batch_refs: Vec<&RecordBatch> = batches.iter().collect();
    json_writer
        .write_batches(&batch_refs)
        .with_context(|| "json_writer.write_batches")?;
    json_writer.finish().unwrap();
    Ok(String::from_utf8(buffer).with_context(|| "converting json buffer to utf8")?)
}

pub fn register_routes(router: Router) -> Router {
    router.route("/gateway/query", post(handle_query))
}
