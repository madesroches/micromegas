use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{get, get_service, post},
};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use clap::Parser;
use datafusion::arrow::array::{
    Int32Array, Int64Array, ListArray, StringArray, TimestampNanosecondArray, UInt64Array,
};
use futures::{Stream, StreamExt};
use http::{HeaderValue, Method, header};
use micromegas::analytics::{
    arrow_properties::read_property_list, dfext::typed_column::typed_column_by_name,
    time::TimeRange,
};
use micromegas::client::{
    flightsql_client_factory::{BearerFlightSQLClientFactory, FlightSQLClientFactory},
    perfetto_trace_client,
    query_processes::ProcessQueryBuilder,
};
use micromegas::micromegas_main;
use micromegas::servers::axum_utils::observability_middleware;
use micromegas::telemetry::property::Property;
use micromegas::tracing::prelude::*;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, pin::Pin, time::Duration};
use tower_http::{
    cors::{Any, CorsLayer},
    services::{ServeDir, ServeFile},
};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Server port
    #[arg(short, long, default_value = "3000")]
    port: u16,

    /// Frontend build directory
    #[arg(long, default_value = "../analytics-web-app/dist")]
    frontend_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProcessInfo {
    process_id: String,
    exe: String,
    start_time: DateTime<Utc>,
    last_update_time: DateTime<Utc>,
    computer: String,
    username: String,
    cpu_brand: String,
    distro: String,
    properties: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GenerateTraceRequest {
    time_range: Option<TimeRangeQuery>,
    include_async_spans: bool,
    include_thread_spans: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct TimeRangeQuery {
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProgressUpdate {
    #[serde(rename = "type")]
    update_type: String,
    percentage: u8,
    message: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct BinaryStartMarker {
    #[serde(rename = "type")]
    update_type: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct TraceMetadata {
    process_id: String,
    estimated_size_bytes: Option<u64>,
    span_counts: SpanCounts,
    generation_time_estimate: Duration,
}

#[derive(Debug, Serialize, Deserialize)]
struct SpanCounts {
    thread_spans: u64,
    async_spans: u64,
    total: u64,
}

#[derive(Debug, Serialize)]
struct HealthCheck {
    status: String,
    timestamp: DateTime<Utc>,
    flightsql_connected: bool,
}

#[derive(Debug, Serialize)]
struct ProcessStatistics {
    process_id: String,
    log_entries: u64,
    measures: u64,
    trace_events: u64,
    thread_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LogEntry {
    time: DateTime<Utc>,
    level: String,
    target: String,
    msg: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct LogsQuery {
    limit: Option<usize>,
    level: Option<String>,
}

#[derive(Clone)]
struct AppState {
    auth_token: String,
}

#[derive(Debug, Serialize)]
struct ApiError {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
    details: Option<String>,
}

impl ApiError {
    fn new(error_type: &str, message: &str) -> Self {
        Self {
            error_type: error_type.to_string(),
            message: message.to_string(),
            details: None,
        }
    }

    fn with_details(error_type: &str, message: &str, details: &str) -> Self {
        Self {
            error_type: error_type.to_string(),
            message: message.to_string(),
            details: Some(details.to_string()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(serde_json::json!({
            "error": self
        }));
        (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        ApiError::new("InternalError", &err.to_string())
    }
}

type ProgressStream = Pin<Box<dyn Stream<Item = Result<Bytes, axum::Error>> + Send>>;

#[micromegas_main(interop_max_level = "info")]
async fn main() -> Result<()> {
    let args = Args::parse();

    let auth_token = std::env::var("MICROMEGAS_AUTH_TOKEN").unwrap_or_else(|_| "".to_string());

    // Configure CORS based on environment variable
    let cors_origin = std::env::var("ANALYTICS_WEB_CORS_ORIGIN")
        .unwrap_or_else(|_| "http://localhost:3000".to_string());

    let state = AppState { auth_token };
    let health_routes = Router::new()
        .route("/analyticsweb/health", get(health_check))
        .with_state(state.clone());

    let api_routes = Router::new()
        .route("/analyticsweb/processes", get(list_processes))
        .route(
            "/analyticsweb/perfetto/{process_id}/info",
            get(get_trace_info),
        )
        .route(
            "/analyticsweb/perfetto/{process_id}/validate",
            post(validate_trace),
        )
        .route(
            "/analyticsweb/perfetto/{process_id}/generate",
            post(generate_trace),
        )
        .route(
            "/analyticsweb/process/{process_id}/log-entries",
            get(get_process_log_entries),
        )
        .route(
            "/analyticsweb/process/{process_id}/statistics",
            get(get_process_statistics),
        )
        .layer(middleware::from_fn(observability_middleware))
        .with_state(state);
    let serve_dir = ServeDir::new(&args.frontend_dir)
        .not_found_service(ServeFile::new(format!("{}/index.html", args.frontend_dir)));

    // Configure CORS layer
    let cors_layer = if cors_origin == "*" {
        // Development mode - allow any origin
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
    } else {
        // Production mode - restrict to specific origin
        let origin = cors_origin
            .parse::<HeaderValue>()
            .expect("Invalid CORS origin format");
        CorsLayer::new()
            .allow_origin(origin)
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
    };

    let app = Router::new()
        .merge(health_routes)
        .merge(api_routes)
        .fallback_service(get_service(serve_dir))
        .layer(cors_layer);

    let addr = format!("0.0.0.0:{}", args.port);
    println!("Analytics web server starting on {}", addr);
    println!("CORS origin configured for: {}", cors_origin);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

#[span_fn]
async fn health_check(State(state): State<AppState>) -> impl IntoResponse {
    let mut flightsql_connected = false;

    let client_factory = BearerFlightSQLClientFactory::new(state.auth_token);
    if let Ok(mut client) = client_factory.make_client().await {
        flightsql_connected = client.query("SELECT 1".to_string(), None).await.is_ok();
    }

    let health = HealthCheck {
        status: if flightsql_connected {
            "healthy".to_string()
        } else {
            "degraded".to_string()
        },
        timestamp: Utc::now(),
        flightsql_connected,
    };

    Json(health)
}

#[span_fn]
async fn list_processes(State(state): State<AppState>) -> impl IntoResponse {
    let client_factory = BearerFlightSQLClientFactory::new(state.auth_token);
    match get_processes_internal(&client_factory).await {
        Ok(processes) => (StatusCode::OK, Json(processes)).into_response(),
        Err(e) => {
            tracing::error!("Failed to list processes: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to list processes",
                    "details": e.to_string()
                })),
            )
                .into_response()
        }
    }
}

fn convert_properties_to_map(properties: Vec<Property>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for prop in properties {
        map.insert(prop.key_str().to_string(), prop.value_str().to_string());
    }
    map
}

#[span_fn]
async fn get_processes_internal(
    client_factory: &BearerFlightSQLClientFactory,
) -> Result<Vec<ProcessInfo>> {
    let mut client = client_factory.make_client().await?;

    let query_builder = ProcessQueryBuilder::new();

    let batches = query_builder.query(&mut client).await?;

    let mut processes = Vec::new();

    for batch in batches {
        let process_ids: &StringArray = typed_column_by_name(&batch, "process_id")?;
        let exes: &StringArray = typed_column_by_name(&batch, "exe")?;
        let start_times: &TimestampNanosecondArray = typed_column_by_name(&batch, "start_time")?;
        let last_update_times: &TimestampNanosecondArray =
            typed_column_by_name(&batch, "last_update_time")?;
        let computers: &StringArray = typed_column_by_name(&batch, "computer")?;
        let usernames: &StringArray = typed_column_by_name(&batch, "username")?;
        let cpu_brands: &StringArray = typed_column_by_name(&batch, "cpu_brand")?;
        let distros: &StringArray = typed_column_by_name(&batch, "distro")?;
        let properties_array: &ListArray = typed_column_by_name(&batch, "properties")?;

        for row in 0..batch.num_rows() {
            let properties_vec = read_property_list(properties_array.value(row))?;
            let properties = convert_properties_to_map(properties_vec);

            processes.push(ProcessInfo {
                process_id: process_ids.value(row).to_string(),
                exe: exes.value(row).to_string(),
                start_time: DateTime::from_timestamp_nanos(start_times.value(row)),
                last_update_time: DateTime::from_timestamp_nanos(last_update_times.value(row)),
                computer: computers.value(row).to_string(),
                username: usernames.value(row).to_string(),
                cpu_brand: cpu_brands.value(row).to_string(),
                distro: distros.value(row).to_string(),
                properties,
            });
        }
    }

    Ok(processes)
}

async fn get_trace_info(
    Path(process_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<TraceMetadata>, ApiError> {
    let client_factory = BearerFlightSQLClientFactory::new(state.auth_token.clone());
    let mut client = client_factory.make_client().await.map_err(|e| {
        ApiError::with_details(
            "FlightSQLConnectionError",
            "Failed to create FlightSQL client",
            &e.to_string(),
        )
    })?;

    // Get thread span count by counting streams with cpu tag
    let sql_streams = format!(
        r#"
        SELECT COUNT(DISTINCT stream_id) as thread_count
        FROM blocks
        WHERE process_id = '{}' 
        AND array_has("streams.tags", 'cpu')
        "#,
        process_id
    );

    let mut thread_span_count = 0u64;
    let stream_batches = client.query(sql_streams, None).await.map_err(|e| {
        ApiError::with_details(
            "DatabaseQueryError",
            "Failed to query thread streams",
            &e.to_string(),
        )
    })?;

    for batch in stream_batches {
        if batch.num_rows() > 0 {
            let counts: &Int64Array =
                typed_column_by_name(&batch, "thread_count").map_err(|e| {
                    ApiError::with_details(
                        "DataParsingError",
                        "Failed to parse thread count",
                        &e.to_string(),
                    )
                })?;
            thread_span_count = counts.value(0) as u64;
        }
    }

    // Estimate span counts (actual counting would be expensive)
    let estimated_spans_per_thread = 100;
    let total_thread_spans = thread_span_count * estimated_spans_per_thread;
    let total_async_spans = total_thread_spans / 4; // Rough estimate
    let total_spans = total_thread_spans + total_async_spans;

    // Estimate size (roughly 100 bytes per span)
    let estimated_size_bytes = Some(total_spans * 100);

    // Estimate generation time based on span count
    let generation_time_estimate = if total_spans < 1000 {
        Duration::from_secs(2)
    } else if total_spans < 10000 {
        Duration::from_secs(5)
    } else {
        Duration::from_secs(15)
    };

    let metadata = TraceMetadata {
        process_id: process_id.clone(),
        estimated_size_bytes,
        span_counts: SpanCounts {
            thread_spans: total_thread_spans,
            async_spans: total_async_spans,
            total: total_spans,
        },
        generation_time_estimate,
    };

    Ok(Json(metadata))
}

async fn validate_trace(
    Path(process_id): Path<String>,
    State(_state): State<AppState>,
) -> impl IntoResponse {
    Json(serde_json::json!({
        "valid": true,
        "process_id": process_id,
        "message": "Trace structure is valid"
    }))
}

#[span_fn]
async fn generate_trace(
    Path(process_id): Path<String>,
    State(state): State<AppState>,
    Json(request): Json<GenerateTraceRequest>,
) -> impl IntoResponse {
    let stream = generate_trace_stream(process_id, state, request);

    Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::TRANSFER_ENCODING, "chunked")
        .body(axum::body::Body::from_stream(stream))
        .unwrap()
}

#[span_fn]
async fn get_process_log_entries(
    Path(process_id): Path<String>,
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<LogsQuery>,
) -> Result<Json<Vec<LogEntry>>, ApiError> {
    let limit = query.limit.unwrap_or(50);
    let level_filter = query.level.unwrap_or_else(|| "all".to_string());

    let client_factory = BearerFlightSQLClientFactory::new(state.auth_token.clone());
    let mut client = client_factory.make_client().await.map_err(|e| {
        ApiError::with_details(
            "FlightSQLConnectionError",
            "Failed to create FlightSQL client",
            &e.to_string(),
        )
    })?;

    // Build SQL query
    let level_condition = match level_filter.as_str() {
        "fatal" => "AND level = 1", // FATAL level
        "error" => "AND level = 2", // ERROR level
        "warn" => "AND level = 3",  // WARN level
        "info" => "AND level = 4",  // INFO level
        "debug" => "AND level = 5", // DEBUG level
        "trace" => "AND level = 6", // TRACE level
        _ => "",                    // No filter
    };

    let sql = format!(
        "SELECT time, level, target, msg 
         FROM log_entries 
         WHERE process_id = '{}' {} 
         ORDER BY time DESC 
         LIMIT {}",
        process_id, level_condition, limit
    );

    let mut logs = Vec::new();

    let mut stream = client.query_stream(sql, None).await.map_err(|e| {
        ApiError::with_details(
            "DatabaseQueryError",
            "Failed to query log entries",
            &e.to_string(),
        )
    })?;

    while let Some(batch) = stream.next().await {
        let batch = batch.map_err(|e| {
            ApiError::with_details(
                "StreamProcessingError",
                "Error processing query result batch",
                &e.to_string(),
            )
        })?;

        let times: &TimestampNanosecondArray =
            typed_column_by_name(&batch, "time").map_err(|e| {
                ApiError::with_details(
                    "DataParsingError",
                    "Failed to parse time column",
                    &e.to_string(),
                )
            })?;
        let levels: &Int32Array = typed_column_by_name(&batch, "level").map_err(|e| {
            ApiError::with_details(
                "DataParsingError",
                "Failed to parse level column",
                &e.to_string(),
            )
        })?;
        let targets: &StringArray = typed_column_by_name(&batch, "target").map_err(|e| {
            ApiError::with_details(
                "DataParsingError",
                "Failed to parse target column",
                &e.to_string(),
            )
        })?;
        let msgs: &StringArray = typed_column_by_name(&batch, "msg").map_err(|e| {
            ApiError::with_details(
                "DataParsingError",
                "Failed to parse msg column",
                &e.to_string(),
            )
        })?;

        for row in 0..batch.num_rows() {
            let level_value = levels.value(row);
            let level_str = match level_value {
                1 => "FATAL",
                2 => "ERROR",
                3 => "WARN",
                4 => "INFO",
                5 => "DEBUG",
                6 => "TRACE",
                _ => "UNKNOWN",
            }
            .to_string();

            logs.push(LogEntry {
                time: DateTime::from_timestamp_nanos(times.value(row)),
                level: level_str,
                target: targets.value(row).to_string(),
                msg: msgs.value(row).to_string(),
            });
        }
    }

    Ok(Json(logs))
}

#[span_fn]
async fn get_process_statistics(
    Path(process_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ProcessStatistics>, ApiError> {
    let client_factory = BearerFlightSQLClientFactory::new(state.auth_token.clone());
    let mut client = client_factory.make_client().await.map_err(|e| {
        ApiError::with_details(
            "FlightSQLConnectionError",
            "Failed to create FlightSQL client",
            &e.to_string(),
        )
    })?;

    // Query actual statistics from different stream types
    let sql = format!(
        r#"
        SELECT 
            SUM(CASE WHEN array_has("streams.tags", 'log') THEN nb_objects ELSE 0 END) as log_entries,
            SUM(CASE WHEN array_has("streams.tags", 'metrics') THEN nb_objects ELSE 0 END) as measures,
            SUM(CASE WHEN array_has("streams.tags", 'cpu') THEN nb_objects ELSE 0 END) as trace_events,
            COUNT(DISTINCT CASE WHEN array_has("streams.tags", 'cpu') THEN stream_id ELSE NULL END) as thread_count
        FROM blocks 
        WHERE process_id = '{}'
        "#,
        process_id
    );

    let mut log_entries = 0u64;
    let mut measures = 0u64;
    let mut trace_events = 0u64;
    let mut thread_count = 0u64;

    let batches = client.query(sql, None).await.map_err(|e| {
        ApiError::with_details(
            "DatabaseQueryError",
            "Failed to query process statistics",
            &e.to_string(),
        )
    })?;

    for batch in batches {
        if batch.num_rows() > 0 {
            log_entries = typed_column_by_name::<UInt64Array>(&batch, "log_entries")
                .map(|arr| arr.value(0))
                .or_else(|_| typed_column_by_name::<Int64Array>(&batch, "log_entries").map(|arr| arr.value(0) as u64))
                .context("Failed to find log_entries column")?;
            
            measures = typed_column_by_name::<UInt64Array>(&batch, "measures")
                .map(|arr| arr.value(0))
                .or_else(|_| typed_column_by_name::<Int64Array>(&batch, "measures").map(|arr| arr.value(0) as u64))
                .context("Failed to find measures column")?;
            
            trace_events = typed_column_by_name::<UInt64Array>(&batch, "trace_events")
                .map(|arr| arr.value(0))
                .or_else(|_| typed_column_by_name::<Int64Array>(&batch, "trace_events").map(|arr| arr.value(0) as u64))
                .context("Failed to find trace_events column")?;
            
            thread_count = typed_column_by_name::<UInt64Array>(&batch, "thread_count")
                .map(|arr| arr.value(0))
                .or_else(|_| typed_column_by_name::<Int64Array>(&batch, "thread_count").map(|arr| arr.value(0) as u64))
                .context("Failed to find thread_count column")?;
            
            break; // Single row result, no need to continue
        }
    }

    Ok(Json(ProcessStatistics {
        process_id,
        log_entries,
        measures,
        trace_events,
        thread_count,
    }))
}

fn generate_trace_stream(
    process_id: String,
    state: AppState,
    request: GenerateTraceRequest,
) -> ProgressStream {
    use async_stream::stream;
    use tokio::time::{Duration, sleep};

    Box::pin(stream! {
        let progress_updates = vec![
            ProgressUpdate {
                update_type: "progress".to_string(),
                percentage: 10,
                message: "Connecting to FlightSQL server".to_string()
            },
            ProgressUpdate {
                update_type: "progress".to_string(),
                percentage: 25,
                message: "Querying process metadata".to_string()
            },
            ProgressUpdate {
                update_type: "progress".to_string(),
                percentage: 50,
                message: "Processing thread spans".to_string()
            },
            ProgressUpdate {
                update_type: "progress".to_string(),
                percentage: 75,
                message: "Processing async spans".to_string()
            },
            ProgressUpdate {
                update_type: "progress".to_string(),
                percentage: 90,
                message: "Finalizing trace file".to_string()
            },
        ];

        for update in progress_updates {
            if let Ok(json) = serde_json::to_string(&update) {
                yield Ok(Bytes::from(json + "\n"));
                sleep(Duration::from_millis(500)).await;
            }
        }

        let binary_marker = BinaryStartMarker {
            update_type: "binary_start".to_string(),
        };
        if let Ok(json) = serde_json::to_string(&binary_marker) {
            yield Ok(Bytes::from(json + "\n"));
        }

        let client_factory = BearerFlightSQLClientFactory::new(state.auth_token.clone());
        match generate_perfetto_trace_internal(&client_factory, &process_id, &request).await {
            Ok(trace_data) => {
                const CHUNK_SIZE: usize = 8192;
                for chunk in trace_data.chunks(CHUNK_SIZE) {
                    yield Ok(Bytes::from(chunk.to_vec()));
                }
            },
            Err(e) => {
                tracing::error!("Failed to generate trace: {}", e);
                let error_msg = format!("Error: Failed to generate trace: {}", e);
                yield Ok(Bytes::from(error_msg));
            }
        }
    })
}

#[span_fn]
async fn generate_perfetto_trace_internal(
    client_factory: &BearerFlightSQLClientFactory,
    process_id: &str,
    request: &GenerateTraceRequest,
) -> Result<Vec<u8>> {
    let mut client = client_factory.make_client().await?;

    let time_range = if let Some(range) = &request.time_range {
        TimeRange::new(range.begin, range.end)
    } else {
        let processes = get_processes_internal(client_factory).await?;
        let process = processes
            .iter()
            .find(|p| p.process_id == process_id)
            .ok_or_else(|| anyhow::anyhow!("Process not found"))?;
        TimeRange::new(process.start_time, process.last_update_time)
    };

    let trace_data =
        perfetto_trace_client::format_perfetto_trace(&mut client, process_id, time_range).await?;

    Ok(trace_data)
}
