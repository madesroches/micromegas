mod auth;
mod queries;
mod screens;
mod stream_query;

use analytics_web_srv::app_db;
use anyhow::{Context, Result};
use auth::{AuthState, AuthToken, OidcClientConfig};
use axum::{
    Extension, Json, Router, ServiceExt,
    extract::{Path, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use clap::Parser;
use datafusion::arrow::array::{Int64Array, TimestampNanosecondArray, UInt64Array};
use futures::{Stream, StreamExt};
use http::{HeaderValue, Method, header};
use micromegas::analytics::{
    dfext::{string_column_accessor::string_column_by_name, typed_column::typed_column_by_name},
    properties::{
        properties_column_accessor::properties_column_by_name,
        utils::extract_properties_from_properties_column,
    },
    time::TimeRange,
};
use micromegas::client::{
    SpanTypes,
    flightsql_client_factory::{BearerFlightSQLClientFactory, FlightSQLClientFactory},
    perfetto_trace_client,
};
use micromegas::micromegas_main;
use micromegas::servers::axum_utils::observability_middleware;
use micromegas::tracing::prelude::*;
// micromegas_auth imports available if needed
#[allow(unused_imports)]
use micromegas_auth::{axum::auth_middleware, types::AuthProvider};
use queries::{query_all_processes, query_nb_trace_events};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, pin::Pin, sync::Arc, time::Duration};
use tower_http::{cors::CorsLayer, services::ServeDir};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Server port
    #[arg(short, long, default_value = "3000", env = "MICROMEGAS_PORT")]
    port: u16,

    /// Frontend build directory
    #[arg(long, default_value = "../analytics-web-app/dist")]
    frontend_dir: String,

    /// Disable authentication (development only)
    #[arg(long)]
    disable_auth: bool,
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

type ApiResult<T> = Result<T, ApiError>;

struct ApiError(anyhow::Error);

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        ApiError(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        error!("API error: {}", self.0);
        let message = self.0.to_string();
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": message })),
        )
            .into_response()
    }
}

type ProgressStream = Pin<Box<dyn Stream<Item = Result<Bytes, axum::Error>> + Send>>;

/// State for serving index.html with injected runtime config
#[derive(Clone)]
struct IndexState {
    frontend_dir: String,
    base_path: String,
}

/// Serve index.html for SPA routing with runtime config injected
///
/// This handler:
/// 1. Reads index.html from the frontend build directory
/// 2. Injects a script tag with runtime configuration (base path)
///
/// With Vite's relative base path ('./'), asset URLs are already relative
/// so no path rewriting is needed. The only modification is injecting
/// the runtime configuration for API calls and navigation.
async fn serve_index_with_config(
    State(state): State<IndexState>,
    _request: axum::extract::Request,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let index_path = format!("{}/index.html", state.frontend_dir);
    let html = tokio::fs::read_to_string(&index_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read index.html: {e}"),
        )
    })?;

    // Build the config script to inject
    let config_script = format!(
        r#"<script>window.__MICROMEGAS_CONFIG__={{basePath:"{}"}}</script>"#,
        state.base_path
    );

    // Inject before </head>
    let modified_html = html.replace("</head>", &format!("{config_script}</head>"));

    Ok((
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        modified_html,
    ))
}

#[micromegas_main(interop_max_level = "info")]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Configure CORS origin (required)
    let cors_origin = std::env::var("MICROMEGAS_WEB_CORS_ORIGIN")
        .context("MICROMEGAS_WEB_CORS_ORIGIN environment variable not set")?;

    // Read base path (required, e.g., "/" or "/micromegas")
    let base_path = std::env::var("MICROMEGAS_BASE_PATH")
        .context("MICROMEGAS_BASE_PATH environment variable not set")?;
    let base_path = base_path.trim_end_matches('/').to_string();
    // Empty string is valid (represents root "/")
    // Non-empty must start with '/'
    if !base_path.is_empty() && !base_path.starts_with('/') {
        anyhow::bail!("MICROMEGAS_BASE_PATH must start with '/' (e.g., '/', '/micromegas')");
    }

    // Connect to micromegas_app database for user-defined screens
    let app_db_pool = if let Ok(conn_string) = std::env::var("MICROMEGAS_APP_SQL_CONNECTION_STRING")
    {
        let pool = sqlx::PgPool::connect(&conn_string)
            .await
            .context("Failed to connect to micromegas_app database")?;

        app_db::execute_migration(pool.clone())
            .await
            .context("Failed to run micromegas_app migrations")?;

        println!("Connected to micromegas_app database");
        Some(pool)
    } else {
        println!(
            "WARNING: MICROMEGAS_APP_SQL_CONNECTION_STRING not set - screens feature disabled"
        );
        None
    };

    // Build auth state if authentication is enabled
    let auth_state = if !args.disable_auth {
        // Load OIDC client configuration
        let oidc_config = OidcClientConfig::from_env()
            .map_err(|e| anyhow::anyhow!("Failed to load OIDC client config: {e}"))?;

        let cookie_domain = std::env::var("MICROMEGAS_COOKIE_DOMAIN").ok();
        let secure_cookies = std::env::var("MICROMEGAS_SECURE_COOKIES")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        // Load secret for signing OAuth state parameters from environment variable
        // This prevents CSRF attacks by ensuring state cannot be tampered with
        // IMPORTANT: Must be the same across all instances in a scaled deployment
        let state_signing_secret = std::env::var("MICROMEGAS_STATE_SECRET")
            .context("MICROMEGAS_STATE_SECRET environment variable not set. Generate a secure random secret (e.g., openssl rand -base64 32)")?
            .into_bytes();

        Some(AuthState {
            oidc_provider: Arc::new(tokio::sync::OnceCell::new()),
            auth_provider: Arc::new(tokio::sync::OnceCell::new()),
            config: oidc_config,
            cookie_domain,
            secure_cookies,
            state_signing_secret,
            base_path: base_path.clone(),
        })
    } else {
        println!("WARNING: Authentication is disabled (--disable-auth)");
        None
    };

    // Build auth routes if authentication is enabled, or stub routes if disabled
    let auth_routes = if let Some(auth_state) = auth_state.as_ref() {
        Router::new()
            .route(&format!("{base_path}/auth/login"), get(auth::auth_login))
            .route(
                &format!("{base_path}/auth/callback"),
                get(auth::auth_callback),
            )
            .route(
                &format!("{base_path}/auth/refresh"),
                post(auth::auth_refresh),
            )
            .route(&format!("{base_path}/auth/logout"), post(auth::auth_logout))
            .route(&format!("{base_path}/auth/me"), get(auth::auth_me))
            .with_state(auth_state.clone())
    } else {
        // Stub auth routes for no-auth mode
        Router::new()
            .route(&format!("{base_path}/auth/me"), get(auth_me_no_auth))
            .route(
                &format!("{base_path}/auth/logout"),
                post(auth_logout_no_auth),
            )
    };

    let health_routes = Router::new().route(&format!("{base_path}/health"), get(health_check));

    let api_routes = Router::new()
        .route(
            &format!("{base_path}/query-stream"),
            post(stream_query::stream_query_handler),
        )
        .route(
            &format!("{base_path}/perfetto/{{process_id}}/info"),
            get(get_trace_info),
        )
        .route(
            &format!("{base_path}/perfetto/{{process_id}}/generate"),
            post(generate_trace),
        )
        .layer(middleware::from_fn(observability_middleware));

    // Apply auth middleware if enabled, otherwise inject a dummy token for no-auth mode
    let api_routes = if let Some(auth_state) = auth_state.clone() {
        api_routes.layer(middleware::from_fn_with_state(
            auth_state,
            auth::cookie_auth_middleware,
        ))
    } else {
        // In no-auth mode, inject a dummy AuthToken so handlers don't fail
        api_routes.layer(Extension(AuthToken(String::new())))
    };

    // Build screen routes if database is available
    let screen_routes = if let Some(pool) = app_db_pool {
        Router::new()
            // Screen types (static)
            .route(
                &format!("{base_path}/screen-types"),
                get(screens::list_screen_types),
            )
            .route(
                &format!("{base_path}/screen-types/{{type_name}}/default"),
                get(screens::get_default_config),
            )
            // Screens CRUD
            .route(
                &format!("{base_path}/screens"),
                get(screens::list_screens).post(screens::create_screen),
            )
            .route(
                &format!("{base_path}/screens/{{name}}"),
                get(screens::get_screen)
                    .put(screens::update_screen)
                    .delete(screens::delete_screen),
            )
            .layer(Extension(pool))
            .layer(middleware::from_fn(observability_middleware))
    } else {
        Router::new()
    };

    // State for serving index.html with injected config
    let index_state = IndexState {
        frontend_dir: args.frontend_dir.clone(),
        base_path: base_path.clone(),
    };

    // Configure CORS layer - always restrict to specific origin
    let origin = cors_origin
        .parse::<HeaderValue>()
        .context("Invalid MICROMEGAS_WEB_CORS_ORIGIN format")?;

    let cors_layer = CorsLayer::new()
        .allow_origin(origin)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
        .allow_credentials(true);

    let mut app = Router::new()
        .merge(health_routes)
        .merge(api_routes)
        .merge(screen_routes);

    // Add auth routes (always - either real or stub)
    app = app.merge(auth_routes);

    // Serve static files from frontend directory under the base path
    // index.html needs special handling to inject runtime config
    // SPA routes fall back to index.html for client-side routing
    //
    // With Vite's relative base path ('./'), all asset URLs are relative,
    // so we don't need to rewrite JS/CSS paths like we did with Next.js.
    let spa_fallback = get(serve_index_with_config).with_state(index_state.clone());
    let serve_dir = ServeDir::new(&args.frontend_dir).fallback(spa_fallback.clone());

    // Build frontend router that handles paths UNDER base_path
    // "/" and "/index.html" need explicit handlers to inject runtime config
    // Other static files and SPA routes go through ServeDir with fallback
    let index_handler = get(serve_index_with_config).with_state(index_state.clone());
    let frontend = Router::new()
        // Explicit "/" route - serves index.html with injected config
        .route("/", index_handler.clone())
        // /index.html also serves index with config
        .route("/index.html", index_handler)
        // All other paths: static files with SPA fallback
        .fallback_service(serve_dir);

    // Mount frontend routes - use merge for root, nest for sub-paths
    let app = if base_path.is_empty() {
        // Root path: merge frontend directly (axum doesn't support nest(""))
        app.merge(frontend)
    } else {
        // Sub-path: nest under base_path - handles /base_path/*
        // Note: nest() does NOT match /base_path/ (with trailing slash), only /base_path/*
        let app = app.nest(&base_path, frontend);

        // Explicitly handle /base_path/ (with trailing slash) - nest() doesn't match this
        let base_path_with_slash = format!("{}/", base_path);
        let index_handler_for_root = get(serve_index_with_config).with_state(index_state);
        let app = app.route(&base_path_with_slash, index_handler_for_root);

        // Add middleware to redirect /base_path -> /base_path/
        let base_path_for_redirect = base_path.clone();
        app.layer(axum::middleware::from_fn(
            move |req: axum::extract::Request, next: axum::middleware::Next| {
                let base_path = base_path_for_redirect.clone();
                async move {
                    let path = req.uri().path();
                    // Redirect exact base_path match to base_path/
                    if path == base_path {
                        let redirect_uri = format!("{}/", base_path);
                        return Redirect::permanent(&redirect_uri).into_response();
                    }
                    next.run(req).await.into_response()
                }
            },
        ))
    };

    // Add CORS layer to the router
    let app = app.layer(cors_layer);

    let addr = format!("0.0.0.0:{}", args.port);
    println!("Analytics web server starting on {}", addr);
    println!("CORS origin configured for: {}", cors_origin);
    if !base_path.is_empty() {
        println!("Base path: {}", base_path);
    }
    if args.disable_auth {
        println!("Authentication: DISABLED");
    } else {
        println!("Authentication: ENABLED");
    }

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(
        listener,
        ServiceExt::<axum::extract::Request>::into_make_service_with_connect_info::<
            std::net::SocketAddr,
        >(app),
    )
    .await?;

    Ok(())
}

#[span_fn]
async fn health_check() -> impl IntoResponse {
    let health = HealthCheck {
        status: "healthy".to_string(),
        timestamp: Utc::now(),
        flightsql_connected: false,
    };

    Json(health)
}

/// Stub /auth/me endpoint for no-auth mode - returns a dummy user
#[derive(Debug, Serialize)]
struct NoAuthUserInfo {
    sub: String,
    email: Option<String>,
    name: Option<String>,
}

async fn auth_me_no_auth() -> impl IntoResponse {
    Json(NoAuthUserInfo {
        sub: "anonymous".to_string(),
        email: Some("anonymous@localhost".to_string()),
        name: Some("Anonymous (No Auth)".to_string()),
    })
}

/// Stub /auth/logout endpoint for no-auth mode
async fn auth_logout_no_auth() -> impl IntoResponse {
    StatusCode::OK
}

#[span_fn]
async fn get_processes_internal(
    client_factory: &BearerFlightSQLClientFactory,
) -> Result<Vec<ProcessInfo>> {
    let mut client = client_factory.make_client().await?;

    let batches = query_all_processes(&mut client).await?;

    let mut processes = Vec::new();

    for batch in batches {
        let process_ids = string_column_by_name(&batch, "process_id")?;
        let exes = string_column_by_name(&batch, "exe")?;
        let start_times: &TimestampNanosecondArray = typed_column_by_name(&batch, "start_time")?;
        let last_update_times: &TimestampNanosecondArray =
            typed_column_by_name(&batch, "last_update_time")?;
        let computers = string_column_by_name(&batch, "computer")?;
        let usernames = string_column_by_name(&batch, "username")?;
        let cpu_brands = string_column_by_name(&batch, "cpu_brand")?;
        let distros = string_column_by_name(&batch, "distro")?;
        let properties_accessor = properties_column_by_name(&batch, "properties")?;

        for row in 0..batch.num_rows() {
            let properties =
                extract_properties_from_properties_column(properties_accessor.as_ref(), row)?;

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
    Extension(auth_token): Extension<AuthToken>,
) -> ApiResult<Json<TraceMetadata>> {
    let client_factory =
        BearerFlightSQLClientFactory::new_with_client_type(auth_token.0, "web".to_string());
    let mut client = client_factory.make_client().await?;

    // Get actual trace event counts from the database
    let mut trace_events = 0u64;

    let span_batches = query_nb_trace_events(&mut client, &process_id).await?;

    for batch in span_batches {
        if batch.num_rows() > 0 {
            trace_events = typed_column_by_name::<UInt64Array>(&batch, "trace_events")
                .map(|arr| arr.value(0))
                .or_else(|_| {
                    typed_column_by_name::<Int64Array>(&batch, "trace_events")
                        .map(|arr| arr.value(0) as u64)
                })?;

            break; // Single row result
        }
    }

    // Calculate realistic size estimate based on actual trace event count
    let estimated_size_bytes = Some(trace_events * 100);

    // Estimate generation time based on actual trace event count
    let generation_time_estimate = if trace_events < 1000 {
        Duration::from_secs(2)
    } else if trace_events < 10000 {
        Duration::from_secs(5)
    } else {
        Duration::from_secs(15)
    };

    let metadata = TraceMetadata {
        process_id: process_id.clone(),
        estimated_size_bytes,
        span_counts: SpanCounts {
            thread_spans: trace_events, // All trace events are from CPU (thread) spans for now
            async_spans: 0,             // No async span distinction yet
            total: trace_events,
        },
        generation_time_estimate,
    };

    Ok(Json(metadata))
}

#[span_fn]
async fn generate_trace(
    Path(process_id): Path<String>,
    Extension(auth_token): Extension<AuthToken>,
    Json(request): Json<GenerateTraceRequest>,
) -> ApiResult<Response<axum::body::Body>> {
    let stream = generate_trace_stream(process_id, auth_token.0, request);
    Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::TRANSFER_ENCODING, "chunked")
        .body(axum::body::Body::from_stream(stream))
        .context("failed to build streaming response")
        .map_err(ApiError::from)
}

fn generate_trace_stream(
    process_id: String,
    auth_token: String,
    request: GenerateTraceRequest,
) -> ProgressStream {
    use async_stream::stream;

    Box::pin(stream! {
        // Send initial progress
        let initial_progress = ProgressUpdate {
            update_type: "progress".to_string(),
            message: "Connecting to analytics server...".to_string()
        };
        if let Ok(json) = serde_json::to_string(&initial_progress) {
            yield Ok(Bytes::from(json + "\n"));
        }

        let client_factory = BearerFlightSQLClientFactory::new_with_client_type(
            auth_token,
            "web".to_string(),
        );

        // Create client and compute time range
        let mut client = match client_factory.make_client().await {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to create client: {}", e);
                yield Ok(Bytes::from(format!("Error: {}", e)));
                return;
            }
        };

        let time_range = if let Some(range) = &request.time_range {
            TimeRange::new(range.begin, range.end)
        } else {
            match get_processes_internal(&client_factory).await {
                Ok(processes) => {
                    match processes.iter().find(|p| p.process_id == process_id) {
                        Some(process) => TimeRange::new(process.start_time, process.last_update_time),
                        None => {
                            yield Ok(Bytes::from("Error: Process not found"));
                            return;
                        }
                    }
                }
                Err(e) => {
                    yield Ok(Bytes::from(format!("Error: {}", e)));
                    return;
                }
            }
        };

        let span_types = match (request.include_thread_spans, request.include_async_spans) {
            (true, true) => SpanTypes::Both,
            (true, false) => SpanTypes::Thread,
            (false, true) => SpanTypes::Async,
            (false, false) => SpanTypes::Thread,
        };

        // Send binary_start - data will stream as it's generated
        let binary_marker = BinaryStartMarker {
            update_type: "binary_start".to_string(),
        };
        if let Ok(json) = serde_json::to_string(&binary_marker) {
            yield Ok(Bytes::from(json + "\n"));
        }

        // Stream chunks directly as they arrive from FlightSQL
        let chunk_stream = perfetto_trace_client::format_perfetto_trace_stream(
            &mut client,
            &process_id,
            time_range,
            span_types,
        );
        tokio::pin!(chunk_stream);

        while let Some(chunk_result) = chunk_stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    yield Ok(Bytes::from(chunk));
                }
                Err(e) => {
                    error!("Failed to generate trace chunk: {}", e);
                    // Note: can't send error after binary data started
                    return;
                }
            }
        }
    })
}
