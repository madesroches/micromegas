//! Errors surfaced by the OTLP ingestion adapter.
//!
//! Variants map onto HTTP status codes at the handler boundary:
//!  - `Parse`        → 400 (malformed protobuf, malformed gzip)
//!  - `Database`     → 503 (transient — client should retry per OTLP/HTTP spec)
//!  - `Storage`      → 503 (transient)
//!
//! 415 (Content-Type / Content-Encoding) and 413 (body limit) are enforced upstream
//! in the axum layer stack before the request reaches the OtelError surface.

use micromegas_ingestion::web_ingestion_service::IngestionServiceError;
use thiserror::Error;

/// OTLP signal name (used purely for diagnostic messages).
#[derive(Debug, Clone, Copy)]
pub enum Signal {
    Logs,
    Metrics,
    Traces,
}

impl Signal {
    pub fn as_str(&self) -> &'static str {
        match self {
            Signal::Logs => "logs",
            Signal::Metrics => "metrics",
            Signal::Traces => "traces",
        }
    }
}

impl std::fmt::Display for Signal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Error)]
pub enum OtelError {
    /// Malformed protobuf, malformed gzip, or unrepresentable input.
    /// Maps to 400.
    #[error("OTLP parse error ({signal}): {message}")]
    Parse { signal: Signal, message: String },

    /// PostgreSQL transient failure. Maps to 503 + Retry-After.
    #[error("OTLP database error ({signal}): {message}")]
    Database { signal: Signal, message: String },

    /// Object-store transient failure. Maps to 503 + Retry-After.
    #[error("OTLP storage error ({signal}): {message}")]
    Storage { signal: Signal, message: String },
}

impl OtelError {
    pub fn signal(&self) -> Signal {
        match self {
            Self::Parse { signal, .. }
            | Self::Database { signal, .. }
            | Self::Storage { signal, .. } => *signal,
        }
    }

    /// gRPC canonical `Code` for the embedded `google.rpc.Status` proto on error responses.
    /// Despite the name, this travels over OTLP/HTTP — the spec just reuses `google.rpc.Status`
    /// (and its gRPC code enum) as the error body format.
    pub fn grpc_code(&self) -> i32 {
        match self {
            // INVALID_ARGUMENT = 3
            Self::Parse { .. } => 3,
            // UNAVAILABLE = 14
            Self::Database { .. } | Self::Storage { .. } => 14,
        }
    }

    /// HTTP status code for the response.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::Parse { .. } => 400,
            Self::Database { .. } | Self::Storage { .. } => 503,
        }
    }

    /// True when the OTLP/HTTP spec marks this status retryable.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Database { .. } | Self::Storage { .. })
    }

    /// Client-facing message for the `google.rpc.Status` body. Strips internal
    /// detail (raw sqlx errors, object-store messages) that the full `Display`
    /// form would otherwise leak — those still get logged server-side via
    /// `error!`. `Parse` keeps its detail so clients can debug malformed
    /// payloads, since prost / decoder messages don't reference server state.
    pub fn public_message(&self) -> String {
        match self {
            Self::Parse { signal, message } => format!("OTLP parse error ({signal}): {message}"),
            Self::Database { signal, .. } => format!("OTLP database error ({signal})"),
            Self::Storage { signal, .. } => format!("OTLP storage error ({signal})"),
        }
    }
}

impl OtelError {
    /// Wraps an `IngestionServiceError` with the OTLP signal of the request that
    /// triggered it. Forces the caller to supply the signal at the conversion
    /// site so the resulting label can't be mismatched against the route.
    pub fn from_ingestion(err: IngestionServiceError, signal: Signal) -> Self {
        match err {
            IngestionServiceError::ParseError(m) => OtelError::Parse { signal, message: m },
            IngestionServiceError::DatabaseError(m) => OtelError::Database { signal, message: m },
            IngestionServiceError::StorageError(m) => OtelError::Storage { signal, message: m },
        }
    }
}
