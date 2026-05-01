//! Errors surfaced by the OTLP ingestion adapter.
//!
//! Variants map onto HTTP status codes at the handler boundary:
//!  - `Parse`        → 400 (malformed protobuf, malformed gzip)
//!  - `Database`     → 503 (transient — client should retry per OTLP/HTTP spec)
//!  - `Storage`      → 503 (transient)
//!
//! 415 (Content-Type / Content-Encoding) and 413 (body limit) are enforced upstream
//! in the axum layer stack before the request reaches the OtelError surface.

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

impl From<micromegas_ingestion::web_ingestion_service::IngestionServiceError> for OtelError {
    fn from(err: micromegas_ingestion::web_ingestion_service::IngestionServiceError) -> Self {
        use micromegas_ingestion::web_ingestion_service::IngestionServiceError as Inner;
        // Signal is not known here — the handler attaches the right context when it bubbles up.
        // We default to Logs and let the surrounding code rewrite via `with_signal`.
        match err {
            Inner::ParseError(m) => OtelError::Parse {
                signal: Signal::Logs,
                message: m,
            },
            Inner::DatabaseError(m) => OtelError::Database {
                signal: Signal::Logs,
                message: m,
            },
            Inner::StorageError(m) => OtelError::Storage {
                signal: Signal::Logs,
                message: m,
            },
        }
    }
}

impl OtelError {
    /// Rewrites the embedded signal label (used at the layer boundary where we know the route).
    pub fn with_signal(mut self, sig: Signal) -> Self {
        match &mut self {
            Self::Parse { signal, .. }
            | Self::Database { signal, .. }
            | Self::Storage { signal, .. } => {
                *signal = sig;
            }
        }
        self
    }
}
