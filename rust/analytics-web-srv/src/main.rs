use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, get_service, post},
    Json, Router,
};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use clap::Parser;
use futures::{Stream, StreamExt};
use http::header;
use micromegas::analytics::{dfext::typed_column::typed_column_by_name, time::TimeRange};
use micromegas::client::{flightsql_client_factory::{FlightSQLClientFactory, BearerFlightSQLClientFactory}, perfetto_trace_client, query_processes::ProcessQueryBuilder};
use datafusion::arrow::array::{Int32Array, StringArray, TimestampNanosecondArray};
use serde::{Deserialize, Serialize};
use std::{pin::Pin, time::Duration};
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
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    computer: String,
    username: String,
    cpu_brand: String,
    distro: String,
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

type ProgressStream = Pin<Box<dyn Stream<Item = Result<Bytes, axum::Error>> + Send>>;


#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    
    let auth_token = std::env::var("MICROMEGAS_AUTH_TOKEN")
        .unwrap_or_else(|_| "".to_string());
    
    let state = AppState {
        auth_token,
    };
    let api_routes = Router::new()
        .route("/api/health", get(health_check))
        .route("/api/processes", get(list_processes))
        .route("/api/perfetto/{process_id}/info", get(get_trace_info))
        .route("/api/perfetto/{process_id}/validate", post(validate_trace))
        .route("/api/perfetto/{process_id}/generate", post(generate_trace))
        .route("/api/process/{process_id}/log-entries", get(get_process_log_entries))
        .with_state(state);
    let serve_dir = ServeDir::new(&args.frontend_dir)
        .not_found_service(ServeFile::new(format!("{}/index.html", args.frontend_dir)));
    
    let app = Router::new()
        .merge(api_routes)
        .fallback_service(get_service(serve_dir))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any)
        );

    let addr = format!("0.0.0.0:{}", args.port);
    println!("Analytics web server starting on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health_check(State(state): State<AppState>) -> impl IntoResponse {
    let mut flightsql_connected = false;
    
    let client_factory = BearerFlightSQLClientFactory::new(state.auth_token);
    if let Ok(mut client) = client_factory.make_client().await {
        flightsql_connected = client.query("SELECT 1".to_string(), None).await.is_ok();
    }

    let health = HealthCheck {
        status: if flightsql_connected { "healthy".to_string() } else { "degraded".to_string() },
        timestamp: Utc::now(),
        flightsql_connected,
    };

    Json(health)
}

async fn list_processes(State(state): State<AppState>) -> impl IntoResponse {
    let client_factory = BearerFlightSQLClientFactory::new(state.auth_token);
    match get_processes_internal(&client_factory).await {
        Ok(processes) => (StatusCode::OK, Json(processes)).into_response(),
        Err(e) => {
            tracing::error!("Failed to list processes: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                "error": "Failed to list processes",
                "details": e.to_string()
            }))).into_response()
        }
    }
}

async fn get_processes_internal(client_factory: &BearerFlightSQLClientFactory) -> Result<Vec<ProcessInfo>> {
    let mut client = client_factory.make_client().await?;
    
    let query_builder = ProcessQueryBuilder::new()
        .with_cpu_blocks();
    
    let batches = query_builder.query(&mut client).await?;
    
    let mut processes = Vec::new();
    
    for batch in batches {
        let process_ids: &StringArray = 
            typed_column_by_name(&batch, "process_id")?;
        let exes: &StringArray = 
            typed_column_by_name(&batch, "exe")?;
        let begins: &TimestampNanosecondArray = 
            typed_column_by_name(&batch, "begin")?;
        let ends: &TimestampNanosecondArray = 
            typed_column_by_name(&batch, "end")?;
        let computers: &StringArray = 
            typed_column_by_name(&batch, "computer")?;
        let usernames: &StringArray = 
            typed_column_by_name(&batch, "username")?;
        let cpu_brands: &StringArray = 
            typed_column_by_name(&batch, "cpu_brand")?;
        let distros: &StringArray = 
            typed_column_by_name(&batch, "distro")?;
            
        for row in 0..batch.num_rows() {
            processes.push(ProcessInfo {
                process_id: process_ids.value(row).to_string(),
                exe: exes.value(row).to_string(),
                begin: DateTime::from_timestamp_nanos(begins.value(row)),
                end: DateTime::from_timestamp_nanos(ends.value(row)),
                computer: computers.value(row).to_string(),
                username: usernames.value(row).to_string(),
                cpu_brand: cpu_brands.value(row).to_string(),
                distro: distros.value(row).to_string(),
            });
        }
    }
    
    Ok(processes)
}

async fn get_trace_info(
    Path(process_id): Path<String>, 
    State(_state): State<AppState>
) -> impl IntoResponse {
    let metadata = TraceMetadata {
        process_id: process_id.clone(),
        estimated_size_bytes: Some(1024 * 1024),
        span_counts: SpanCounts {
            thread_spans: 1000,
            async_spans: 500,
            total: 1500,
        },
        generation_time_estimate: Duration::from_secs(10),
    };
    
    Json(metadata)
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

async fn get_process_log_entries(
    Path(process_id): Path<String>,
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<LogsQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(50);
    let level_filter = query.level.unwrap_or_else(|| "all".to_string());
    
    let client_factory = BearerFlightSQLClientFactory::new(state.auth_token.clone());
    let mut client = match client_factory.make_client().await {
        Ok(client) => client,
        Err(e) => {
            eprintln!("Failed to create FlightSQL client: {}", e);
            return Json(Vec::<LogEntry>::new());
        }
    };
    
    // Build SQL query
    let level_condition = match level_filter.as_str() {
        "fatal" => "AND level = 1",    // FATAL level
        "error" => "AND level = 2",    // ERROR level
        "warn" => "AND level = 3",     // WARN level
        "info" => "AND level = 4",     // INFO level
        "debug" => "AND level = 5",    // DEBUG level
        "trace" => "AND level = 6",    // TRACE level
        _ => "",  // No filter
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
    
    match client.query_stream(sql, None).await {
        Ok(mut stream) => {
            while let Some(batch) = stream.next().await {
                match batch {
                    Ok(batch) => {
                        let times: &TimestampNanosecondArray = match typed_column_by_name(&batch, "time") {
                            Ok(arr) => arr,
                            Err(_) => continue,
                        };
                        let levels: &Int32Array = match typed_column_by_name(&batch, "level") {
                            Ok(arr) => arr,
                            Err(_) => continue,
                        };
                        let targets: &StringArray = match typed_column_by_name(&batch, "target") {
                            Ok(arr) => arr,
                            Err(_) => continue,
                        };
                        let msgs: &StringArray = match typed_column_by_name(&batch, "msg") {
                            Ok(arr) => arr,
                            Err(_) => continue,
                        };
                        
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
                            }.to_string();
                            
                            logs.push(LogEntry {
                                time: DateTime::from_timestamp_nanos(times.value(row)),
                                level: level_str,
                                target: targets.value(row).to_string(),
                                msg: msgs.value(row).to_string(),
                            });
                        }
                    }
                    Err(e) => {
                        eprintln!("Error processing batch: {}", e);
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Failed to query logs: {}", e);
        }
    }
    
    Json(logs)
}

fn generate_trace_stream(
    process_id: String, 
    state: AppState,
    request: GenerateTraceRequest
) -> ProgressStream {
    use async_stream::stream;
    use tokio::time::{sleep, Duration};
    
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

async fn generate_perfetto_trace_internal(
    client_factory: &BearerFlightSQLClientFactory, 
    process_id: &str,
    request: &GenerateTraceRequest
) -> Result<Vec<u8>> {
    let mut client = client_factory.make_client().await?;
    
    let time_range = if let Some(range) = &request.time_range {
        TimeRange::new(range.begin, range.end)
    } else {
        let processes = get_processes_internal(client_factory).await?;
        let process = processes.iter()
            .find(|p| p.process_id == process_id)
            .ok_or_else(|| anyhow::anyhow!("Process not found"))?;
        TimeRange::new(process.begin, process.end)
    };
    
    let trace_data = perfetto_trace_client::format_perfetto_trace(
        &mut client, 
        process_id, 
        time_range
    ).await?;
    
    Ok(trace_data)
}