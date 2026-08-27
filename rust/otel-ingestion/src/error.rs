//! Errors surfaced by the OTLP ingestion adapter.
//!
//! Variants map onto HTTP status codes at the handler boundary:
//!  - `Parse`        → 400 (malformed protobuf, malformed gzip)
//!  - `Database`     → 503 (transient — client should retry per OTLP/HTTP spec)
//!  - `Storage`      → 503 (transient)
//!  - `Denied`       → 403 (AbAC Stage 5, #1373 and Stage 5b, #1518: a conflicting
//!    process or stream re-registration under a different audience)
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

    /// A conflicting re-registration under a different audience (AbAC Stage 5, #1373). Maps to
    /// 403 -- never retryable, since retrying with the same credential produces the same
    /// denial.
    #[error("OTLP write denied ({signal}): {message}")]
    Denied { signal: Signal, message: String },
}

impl OtelError {
    pub fn signal(&self) -> Signal {
        match self {
            Self::Parse { signal, .. }
            | Self::Database { signal, .. }
            | Self::Storage { signal, .. }
            | Self::Denied { signal, .. } => *signal,
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
            // PERMISSION_DENIED = 7
            Self::Denied { .. } => 7,
        }
    }

    /// HTTP status code for the response.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::Parse { .. } => 400,
            Self::Database { .. } | Self::Storage { .. } => 503,
            Self::Denied { .. } => 403,
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
    ///
    /// Deliberately source-neutral (no "OTLP" prefix): this surface is also
    /// reused by the webhook route, whose clients never spoke OTLP and would be
    /// confused by OTLP-branded wording. The `Display` form keeps "OTLP" for
    /// server-side logs.
    pub fn public_message(&self) -> String {
        match self {
            Self::Parse { signal, message } => format!("parse error ({signal}): {message}"),
            Self::Database { signal, .. } => format!("database error ({signal})"),
            Self::Storage { signal, .. } => format!("storage error ({signal})"),
            // Sanitized: no internal detail (audience labels, process/stream ids) leaks to the
            // client. Covers both the process- and stream-side conflict guards (AbAC Stage 5
            // §6, Stage 5b §5) -- either can produce this variant.
            Self::Denied { .. } => {
                "write denied: already registered under a different audience".to_string()
            }
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
            // Reachable: `register_otel_process` runs the same conflict guard as the native
            // `insert_process` path (`web_ingestion_service.rs`'s doc comment, AbAC Stage 5 §6),
            // and rejects a same-`process_id`, different-audience OTLP registration with this
            // variant -- closing cross-path squatting where a credential pre-registers (via
            // `insert_process`) the `process_id` a victim's OTLP producer would later derive.
            IngestionServiceError::AudienceConflict {
                process_id,
                existing,
                incoming,
            } => OtelError::Denied {
                signal,
                message: format!(
                    "process_id {process_id} was registered under audience {existing:?}, this \
                     request carries {incoming:?}"
                ),
            },
            // Reachable the same way: `register_otel_stream` runs the stream-side conflict
            // guard (AbAC Stage 5b, #1518 §5), rejecting a same-`stream_id`, different-audience
            // re-registration -- the stream-side mirror of the `AudienceConflict` arm above.
            IngestionServiceError::StreamAudienceConflict {
                stream_id,
                existing,
                incoming,
            } => OtelError::Denied {
                signal,
                message: format!(
                    "stream_id {stream_id} was registered under audience {existing:?}, this \
                     request carries {incoming:?}"
                ),
            },
        }
    }

    /// Prepends a diagnostic prefix (e.g. `firehose record[i] message[j]`) to the
    /// error's message, preserving its variant (and therefore HTTP status / retryability).
    /// Used to localize which record/message failed within a multi-message batch.
    pub fn with_context(self, prefix: impl std::fmt::Display) -> Self {
        match self {
            Self::Parse { signal, message } => Self::Parse {
                signal,
                message: format!("{prefix}: {message}"),
            },
            Self::Database { signal, message } => Self::Database {
                signal,
                message: format!("{prefix}: {message}"),
            },
            Self::Storage { signal, message } => Self::Storage {
                signal,
                message: format!("{prefix}: {message}"),
            },
            Self::Denied { signal, message } => Self::Denied {
                signal,
                message: format!("{prefix}: {message}"),
            },
        }
    }
}
