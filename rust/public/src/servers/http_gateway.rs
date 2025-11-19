use anyhow::{Context, Result};
use axum::{
    Extension, Json, Router,
    body::Body,
    extract::ConnectInfo,
    http::{Response, StatusCode},
    response::IntoResponse,
    routing::post,
};
use chrono::{DateTime, Utc};
use datafusion::arrow::{
    array::RecordBatch,
    json::{Writer, writer::JsonArray},
};
use http::{HeaderMap, Uri};
use micromegas_analytics::time::TimeRange;
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
    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Forbidden: {0}")]
    Forbidden(String),

    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),

    #[error("Internal server error: {0}")]
    Internal(String),
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response<Body> {
        let (status, message) = match self {
            GatewayError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            GatewayError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
            GatewayError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
            GatewayError::ServiceUnavailable(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg),
            GatewayError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, message).into_response()
    }
}

#[derive(Debug, Deserialize)]
pub struct QueryRequest {
    sql: String,
    /// Optional time range filter - begin timestamp in RFC3339 format
    /// Example: "2024-01-01T00:00:00Z"
    #[serde(default)]
    time_range_begin: Option<String>,
    /// Optional time range filter - end timestamp in RFC3339 format
    /// Example: "2024-01-02T00:00:00Z"
    #[serde(default)]
    time_range_end: Option<String>,
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
    let start_time = std::time::Instant::now();

    // Build origin tracking metadata
    let origin_metadata = build_origin_metadata(&headers, &addr);
    let client_type_header = origin_metadata
        .get("x-client-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown+gateway");
    let request_id_header = origin_metadata
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");

    // Request validation
    let sql = request.sql.trim();
    if sql.is_empty() {
        return Err(GatewayError::BadRequest(
            "SQL query cannot be empty".to_string(),
        ));
    }

    // Basic size limit (1MB for SQL query)
    const MAX_SQL_SIZE: usize = 1_048_576;
    if sql.len() > MAX_SQL_SIZE {
        return Err(GatewayError::BadRequest(format!(
            "SQL query too large: {} bytes (max: {} bytes)",
            sql.len(),
            MAX_SQL_SIZE
        )));
    }

    // Parse time range if provided
    let time_range = match (&request.time_range_begin, &request.time_range_end) {
        (Some(begin_str), Some(end_str)) => {
            let begin = DateTime::parse_from_rfc3339(begin_str)
                .map_err(|e| {
                    GatewayError::BadRequest(format!(
                        "Invalid time_range_begin format (expected RFC3339): {e}"
                    ))
                })?
                .with_timezone(&Utc);
            let end = DateTime::parse_from_rfc3339(end_str)
                .map_err(|e| {
                    GatewayError::BadRequest(format!(
                        "Invalid time_range_end format (expected RFC3339): {e}"
                    ))
                })?
                .with_timezone(&Utc);

            if begin > end {
                return Err(GatewayError::BadRequest(
                    "time_range_begin must be before time_range_end".to_string(),
                ));
            }

            Some(TimeRange::new(begin, end))
        }
        (Some(_), None) => {
            return Err(GatewayError::BadRequest(
                "time_range_end must be provided when time_range_begin is specified".to_string(),
            ));
        }
        (None, Some(_)) => {
            return Err(GatewayError::BadRequest(
                "time_range_begin must be provided when time_range_end is specified".to_string(),
            ));
        }
        (None, None) => None,
    };

    info!(
        "Gateway request: request_id={}, client_type={}, time_range={:?}, sql={}",
        request_id_header, client_type_header, time_range, sql
    );

    // Connect to FlightSQL backend
    let flight_url = std::env::var("MICROMEGAS_FLIGHTSQL_URL")
        .map_err(|_| GatewayError::Internal("MICROMEGAS_FLIGHTSQL_URL not configured".to_string()))?
        .parse::<Uri>()
        .map_err(|e| GatewayError::Internal(format!("Invalid FlightSQL URL: {e}")))?;

    let tls_config = ClientTlsConfig::new().with_native_roots();
    let channel = Channel::builder(flight_url)
        .tls_config(tls_config)
        .map_err(|e| GatewayError::Internal(format!("TLS config error: {e}")))?
        .connect()
        .await
        .map_err(|e| {
            GatewayError::ServiceUnavailable(format!("Failed to connect to FlightSQL: {e}"))
        })?;

    // Create client and set headers
    let mut client = Client::new(channel);

    client
        .inner_mut()
        .set_header("x-client-type", client_type_header);
    client
        .inner_mut()
        .set_header("x-request-id", request_id_header);

    if let Some(client_ip) = origin_metadata.get("x-client-ip")
        && let Ok(ip_str) = client_ip.to_str()
    {
        client.inner_mut().set_header("x-client-ip", ip_str);
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

        if config.should_forward(header_name)
            && let Ok(value_str) = value.to_str()
        {
            client.inner_mut().set_header(header_name, value_str);
        }
    }

    // Execute query with error handling
    let batches = client
        .query(sql.to_string(), time_range)
        .await
        .map_err(|e| {
            // Map tonic errors to appropriate HTTP status codes
            if let Some(status) = e.downcast_ref::<tonic::Status>() {
                match status.code() {
                    tonic::Code::Unauthenticated => {
                        GatewayError::Unauthorized(status.message().to_string())
                    }
                    tonic::Code::PermissionDenied => {
                        GatewayError::Forbidden(status.message().to_string())
                    }
                    tonic::Code::InvalidArgument => {
                        GatewayError::BadRequest(status.message().to_string())
                    }
                    tonic::Code::Unavailable => {
                        GatewayError::ServiceUnavailable(status.message().to_string())
                    }
                    _ => GatewayError::Internal(format!("Query failed: {}", status.message())),
                }
            } else {
                GatewayError::Internal(format!("Query execution error: {e:?}"))
            }
        })?;

    let elapsed = start_time.elapsed();
    info!(
        "Gateway request completed: request_id={}, duration={:?}",
        request_id_header, elapsed
    );

    if batches.is_empty() {
        return Ok("[]".to_string());
    }

    let mut buffer = Vec::new();
    let mut json_writer = Writer::<_, JsonArray>::new(&mut buffer);
    let batch_refs: Vec<&RecordBatch> = batches.iter().collect();
    json_writer
        .write_batches(&batch_refs)
        .map_err(|e| GatewayError::Internal(format!("Failed to serialize results: {e}")))?;
    json_writer
        .finish()
        .map_err(|e| GatewayError::Internal(format!("Failed to finish JSON output: {e}")))?;

    String::from_utf8(buffer)
        .map_err(|e| GatewayError::Internal(format!("Invalid UTF-8 in results: {e}")))
}

pub fn register_routes(router: Router) -> Router {
    router.route("/gateway/query", post(handle_query))
}
