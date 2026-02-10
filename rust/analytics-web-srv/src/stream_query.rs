//! Arrow IPC streaming query endpoint
//!
//! Provides a streaming query endpoint that streams RecordBatches as Arrow IPC messages.
//! Uses a JSON-framed protocol for the frontend to parse.

use crate::auth::AuthToken;
use crate::data_source_cache::DataSourceCache;
use anyhow::{Context, Result};
use arrow_ipc::writer::{CompressionContext, IpcDataGenerator, IpcWriteOptions, write_message};
use async_stream::stream;
use axum::{
    Extension, Json,
    body::Body,
    http::{StatusCode, header},
    response::IntoResponse,
};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use datafusion::arrow::datatypes::Schema;
use futures::StreamExt;
use micromegas::analytics::time::TimeRange;
use micromegas::client::flightsql_client_factory::{
    BearerFlightSQLClientFactory, FlightSQLClientFactory,
};
use micromegas::tracing::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Request for streaming SQL query
#[derive(Debug, Serialize, Deserialize)]
pub struct StreamQueryRequest {
    pub sql: String,
    #[serde(default)]
    pub params: HashMap<String, String>,
    pub begin: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
    #[serde(default)]
    pub data_source: String,
}

/// Error codes for stream query errors
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidSql,
    ConnectionFailed,
    Internal,
    Forbidden,
    DataSourceNotFound,
}

/// Schema and batch frames use identical structure - size-prefixed binary
#[derive(Serialize)]
struct DataHeader {
    #[serde(rename = "type")]
    frame_type: &'static str,
    size: usize,
}

/// Done frame to indicate successful completion
#[derive(Serialize)]
struct DoneFrame {
    #[serde(rename = "type")]
    frame_type: &'static str,
}

/// Error frame for streaming errors
#[derive(Serialize)]
struct ErrorFrame {
    #[serde(rename = "type")]
    frame_type: &'static str,
    code: ErrorCode,
    message: String,
}

/// Serialize a value to a JSON line (with trailing newline)
fn json_line<T: Serialize>(value: &T) -> Bytes {
    let mut json = serde_json::to_string(value).expect("serialization failed");
    json.push('\n');
    Bytes::from(json)
}

/// List of destructive functions that should be blocked in web queries
const BLOCKED_FUNCTIONS: &[&str] = &[
    "retire_partitions",
    "retire_partition_by_metadata",
    "retire_partition_by_file",
];

/// Check if the SQL query contains any blocked destructive functions
pub fn contains_blocked_function(sql: &str) -> Option<&'static str> {
    let sql_lower = sql.to_lowercase();
    BLOCKED_FUNCTIONS
        .iter()
        .find(|&func| sql_lower.contains(func))
        .copied()
}

/// Substitute macro variables in SQL query
pub fn substitute_macros(sql: &str, params: &HashMap<String, String>) -> String {
    let mut result = sql.to_string();
    for (key, value) in params {
        // Escape single quotes in values to prevent SQL injection
        let escaped_value = value.replace('\'', "''");
        // Replace $key with the escaped value
        result = result.replace(&format!("${key}"), &escaped_value);
    }
    result
}

/// Encode a schema to Arrow IPC format
///
/// The tracker should be the same instance used for subsequent batch encoding
/// to ensure dictionary IDs are consistent between schema and batches.
pub fn encode_schema(
    schema: &Schema,
    tracker: &mut arrow_ipc::writer::DictionaryTracker,
) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let data_gen = IpcDataGenerator::default();
    let options = IpcWriteOptions::default();

    let encoded = data_gen.schema_to_bytes_with_dictionary_tracker(schema, tracker, &options);
    write_message(&mut buffer, encoded, &options).context("writing schema message")?;
    Ok(buffer)
}

/// Encode a RecordBatch to Arrow IPC format
pub fn encode_batch(
    batch: &datafusion::arrow::array::RecordBatch,
    tracker: &mut arrow_ipc::writer::DictionaryTracker,
    compression: &mut CompressionContext,
) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let data_gen = IpcDataGenerator::default();
    let options = IpcWriteOptions::default();

    let (encoded_dicts, encoded_batch) = data_gen
        .encode(batch, tracker, &options, compression)
        .context("encoding batch")?;

    // Write dictionary batches first (if any)
    for dict in encoded_dicts {
        write_message(&mut buffer, dict, &options).context("writing dictionary message")?;
    }

    // Write the main batch
    write_message(&mut buffer, encoded_batch, &options).context("writing batch message")?;

    Ok(buffer)
}

/// Streaming SQL query endpoint using Arrow IPC protocol
///
/// Returns a stream of JSON-framed Arrow IPC messages:
/// - `{"type":"schema","size":N}\n` followed by N bytes of schema IPC
/// - `{"type":"batch","size":N}\n` followed by N bytes of batch IPC
/// - `{"type":"done"}\n` on success
/// - `{"type":"error","code":"..","message":"..}\n` on error
#[span_fn]
pub async fn stream_query_handler(
    Extension(auth_token): Extension<AuthToken>,
    Extension(cache): Extension<DataSourceCache>,
    Json(request): Json<StreamQueryRequest>,
) -> impl IntoResponse {
    info!(
        "stream query sql={} params={:?} begin={:?} end={:?} data_source={}",
        request.sql, request.params, request.begin, request.end, request.data_source
    );

    // Check for blocked functions first (before starting the stream)
    if let Some(blocked_func) = contains_blocked_function(&request.sql) {
        return (
            StatusCode::FORBIDDEN,
            Json(ErrorFrame {
                frame_type: "error",
                code: ErrorCode::Forbidden,
                message: format!(
                    "The function '{blocked_func}' is not allowed in web queries for security reasons",
                ),
            }),
        )
            .into_response();
    }

    // Resolve data source
    if request.data_source.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorFrame {
                frame_type: "error",
                code: ErrorCode::DataSourceNotFound,
                message: "No data source specified".to_string(),
            }),
        )
            .into_response();
    }

    let data_source_config = match cache.resolve(&request.data_source).await {
        Ok(Some(config)) => config,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorFrame {
                    frame_type: "error",
                    code: ErrorCode::DataSourceNotFound,
                    message: format!("Data source '{}' not found", request.data_source),
                }),
            )
                .into_response();
        }
        Err(e) => {
            error!("Failed to resolve data source '{}': {e}", request.data_source);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorFrame {
                    frame_type: "error",
                    code: ErrorCode::Internal,
                    message: "Failed to resolve data source".to_string(),
                }),
            )
                .into_response();
        }
    };

    let flightsql_url = data_source_config.url;

    // Substitute macros
    let sql = substitute_macros(&request.sql, &request.params);

    // Build time range if provided
    let time_range = match (request.begin, request.end) {
        (Some(begin), Some(end)) => Some(TimeRange::new(begin, end)),
        _ => None,
    };

    let stream = stream! {
        // Create FlightSQL client
        let client_factory = BearerFlightSQLClientFactory::new_with_client_type(
            flightsql_url,
            auth_token.0,
            "web".to_string(),
        );
        let mut client = match client_factory.make_client().await {
            Ok(c) => c,
            Err(e) => {
                yield Ok::<_, std::io::Error>(json_line(&ErrorFrame {
                    frame_type: "error",
                    code: ErrorCode::ConnectionFailed,
                    message: e.to_string(),
                }));
                return;
            }
        };

        // Start streaming query
        let mut batch_stream = match client.query_stream(sql, time_range).await {
            Ok(s) => s,
            Err(e) => {
                yield Ok(json_line(&ErrorFrame {
                    frame_type: "error",
                    code: ErrorCode::InvalidSql,
                    message: e.to_string(),
                }));
                return;
            }
        };

        // FlightSQL limitation: The schema is embedded in the first FlightData message,
        // not sent separately. The FlightRecordBatchStream only populates its schema field
        // after we read the first message. We must read the first batch here, extract the
        // schema, send it to the client, then process this batch along with the rest.
        let first_batch = batch_stream.next().await;

        // Get schema from the stream (now available after reading first message)
        let schema = match batch_stream.schema() {
            Some(s) => s.clone(),
            None => {
                yield Ok(json_line(&ErrorFrame {
                    frame_type: "error",
                    code: ErrorCode::Internal,
                    message: "No schema in response".to_string(),
                }));
                return;
            }
        };

        // Track dictionaries and compression across schema and batches
        // Must use same tracker for schema and batches to ensure dictionary IDs align
        let mut dict_tracker = arrow_ipc::writer::DictionaryTracker::new(false);
        let mut compression = CompressionContext::default();

        // Encode and send schema
        let schema_bytes = match encode_schema(&schema, &mut dict_tracker) {
            Ok(bytes) => bytes,
            Err(e) => {
                yield Ok(json_line(&ErrorFrame {
                    frame_type: "error",
                    code: ErrorCode::Internal,
                    message: format!("{e:#}"),
                }));
                return;
            }
        };

        yield Ok(json_line(&DataHeader {
            frame_type: "schema",
            size: schema_bytes.len(),
        }));
        yield Ok(Bytes::from(schema_bytes));

        // Helper to encode and yield a batch
        macro_rules! yield_batch {
            ($batch:expr) => {
                let batch_bytes = match encode_batch(&$batch, &mut dict_tracker, &mut compression) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        yield Ok(json_line(&ErrorFrame {
                            frame_type: "error",
                            code: ErrorCode::Internal,
                            message: format!("{e:#}"),
                        }));
                        return;
                    }
                };
                yield Ok(json_line(&DataHeader {
                    frame_type: "batch",
                    size: batch_bytes.len(),
                }));
                yield Ok(Bytes::from(batch_bytes));
            };
        }

        // Process the first batch we read earlier (to get the schema)
        if let Some(result) = first_batch {
            match result {
                Ok(batch) => {
                    yield_batch!(batch);
                }
                Err(e) => {
                    yield Ok(json_line(&ErrorFrame {
                        frame_type: "error",
                        code: ErrorCode::Internal,
                        message: e.to_string(),
                    }));
                    return;
                }
            }
        }

        // Stream remaining batches
        while let Some(result) = batch_stream.next().await {
            match result {
                Ok(batch) => {
                    yield_batch!(batch);
                }
                Err(e) => {
                    yield Ok(json_line(&ErrorFrame {
                        frame_type: "error",
                        code: ErrorCode::Internal,
                        message: e.to_string(),
                    }));
                    return;
                }
            }
        }

        // Success
        yield Ok(json_line(&DoneFrame { frame_type: "done" }));
    };

    (
        [(
            header::CONTENT_TYPE,
            "application/x-micromegas-arrow-stream",
        )],
        Body::from_stream(stream),
    )
        .into_response()
}
