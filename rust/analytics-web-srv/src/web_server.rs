//! Extracted web-server builder used by both the standalone binary and the monolith.

use crate::app_db;
use crate::auth::{AuthState, AuthToken, OidcClientConfig, ValidatedUser};
use crate::data_source_cache::DataSourceCache;
use crate::maps;
use crate::stream_query;
use crate::{data_sources, screens};
use anyhow::{Context, Result};
use axum::{
    Extension, Json, Router, ServiceExt,
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Redirect},
    routing::{get, post, put},
};
use chrono::{DateTime, Utc};
use http::{HeaderValue, Method, header};
use micromegas::servers::axum_utils::observability_middleware;
use micromegas::servers::shutdown::serve_axum_with_graceful_shutdown;
use micromegas::tracing::prelude::*;
use serde::Serialize;
use sqlx::PgPool;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, services::ServeDir};

/// Configuration for the analytics web server.
pub struct WebServerConfig {
    /// HTTP port to listen on (default 3000).
    pub port: u16,
    /// Path to the built frontend `dist/` directory.
    pub frontend_dir: String,
    /// Base path for the app (e.g. `""` or `"/micromegas"`).
    pub base_path: String,
    /// CORS allowed origin (e.g. `"http://localhost:3000"`).
    pub cors_origin: String,
    /// PostgreSQL connection string for the web-app database.
    pub app_db_string: String,
    /// Optional object-store URI for maps blobs.
    pub maps_uri: Option<String>,
    /// Optional override for the maximum maps upload size in bytes.
    pub max_upload_bytes: Option<usize>,
    /// Disable OIDC/cookie auth (anonymous access, development only).
    pub disable_auth: bool,
    /// Environment variable name used to load the OIDC admin list.
    ///
    /// Standalone binary sets this to `"MICROMEGAS_ADMINS"`.
    /// The monolith sets it to the analytics-scoped var (with fallback already resolved).
    pub admin_var_name: String,
}

// ---------------------------------------------------------------------------
// Auth setup
// ---------------------------------------------------------------------------

fn build_auth_state(config: &WebServerConfig) -> Result<Option<AuthState>> {
    let oidc_config = OidcClientConfig::from_env()
        .map_err(|e| anyhow::anyhow!("Failed to load OIDC client config: {e}"))?;

    let cookie_domain = std::env::var("MICROMEGAS_COOKIE_DOMAIN").ok();
    let secure_cookies = std::env::var("MICROMEGAS_SECURE_COOKIES")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    let state_signing_secret = std::env::var("MICROMEGAS_STATE_SECRET")
        .context("MICROMEGAS_STATE_SECRET environment variable not set. Generate a secure random secret (e.g., openssl rand -base64 32)")?
        .into_bytes();

    Ok(Some(AuthState {
        oidc_provider: Arc::new(tokio::sync::OnceCell::new()),
        auth_provider: Arc::new(tokio::sync::OnceCell::new()),
        config: oidc_config,
        cookie_domain,
        secure_cookies,
        state_signing_secret,
        base_path: config.base_path.clone(),
        admin_var_name: config.admin_var_name.clone(),
    }))
}

fn build_auth_routes(base_path: &str, auth_state: &Option<AuthState>) -> Router {
    if let Some(state) = auth_state {
        Router::new()
            .route(
                &format!("{base_path}/auth/login"),
                get(crate::auth::auth_login),
            )
            .route(
                &format!("{base_path}/auth/callback"),
                get(crate::auth::auth_callback),
            )
            .route(
                &format!("{base_path}/auth/refresh"),
                post(crate::auth::auth_refresh),
            )
            .route(
                &format!("{base_path}/auth/logout"),
                post(crate::auth::auth_logout),
            )
            .route(&format!("{base_path}/auth/me"), get(crate::auth::auth_me))
            .with_state(state.clone())
    } else {
        Router::new()
            .route(&format!("{base_path}/auth/me"), get(auth_me_no_auth))
            .route(
                &format!("{base_path}/auth/logout"),
                post(auth_logout_no_auth),
            )
    }
}

// ---------------------------------------------------------------------------
// Readiness state
// ---------------------------------------------------------------------------

struct ReadinessState {
    pool: PgPool,
    ready_ok_until: std::sync::Mutex<Option<std::time::Instant>>,
}

impl ReadinessState {
    fn new(pool: PgPool) -> Self {
        Self {
            pool,
            ready_ok_until: std::sync::Mutex::new(None),
        }
    }

    async fn check_ready(&self) -> bool {
        let now = std::time::Instant::now();
        {
            let guard = self.ready_ok_until.lock().expect("readiness cache lock");
            if let Some(ok_until) = *guard
                && ok_until > now
            {
                return true;
            }
        }

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            sqlx::query("SELECT 1").execute(&self.pool),
        )
        .await;

        match result {
            Ok(Ok(_)) => {
                let mut guard = self.ready_ok_until.lock().expect("readiness cache lock");
                *guard = Some(std::time::Instant::now() + std::time::Duration::from_secs(1));
                true
            }
            _ => {
                let mut guard = self.ready_ok_until.lock().expect("readiness cache lock");
                *guard = None;
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------
// API routes
// ---------------------------------------------------------------------------

fn build_public_routes(base_path: &str, readiness_state: Arc<ReadinessState>) -> Router {
    Router::new()
        .route(&format!("{base_path}/api/health"), get(health_check))
        .route(&format!("{base_path}/api/ready"), get(ready_check))
        .layer(Extension(readiness_state))
}

fn build_protected_routes(
    base_path: &str,
    auth_state: &Option<AuthState>,
    app_db_pool: PgPool,
    data_source_cache: DataSourceCache,
    maps_state: maps::MapsState,
) -> Router {
    let routes = Router::new()
        .route(
            &format!("{base_path}/api/query-stream"),
            post(stream_query::stream_query_handler),
        )
        .route(
            &format!("{base_path}/api/screen-types"),
            get(screens::list_screen_types),
        )
        .route(
            &format!("{base_path}/api/screen-types/{{type_name}}/default"),
            get(screens::get_default_config),
        )
        .route(
            &format!("{base_path}/api/screens"),
            get(screens::list_screens).post(screens::create_screen),
        )
        .route(
            &format!("{base_path}/api/screens/{{name}}"),
            get(screens::get_screen)
                .put(screens::update_screen)
                .delete(screens::delete_screen),
        )
        .route(
            &format!("{base_path}/api/data-sources"),
            get(data_sources::list_data_sources).post(data_sources::create_data_source),
        )
        .route(
            &format!("{base_path}/api/data-sources/{{name}}"),
            get(data_sources::get_data_source)
                .put(data_sources::update_data_source)
                .delete(data_sources::delete_data_source),
        )
        .route(
            &format!("{base_path}/api/maps/catalog"),
            get(maps::maps_catalog),
        )
        .route(
            &format!("{base_path}/api/maps/blob/{{filename}}"),
            put(maps::maps_upload)
                .layer(DefaultBodyLimit::max(maps_state.max_upload_bytes))
                .delete(maps::maps_delete),
        )
        .layer(Extension(app_db_pool))
        .layer(Extension(data_source_cache))
        .layer(Extension(maps_state))
        .layer(middleware::from_fn(observability_middleware));

    if let Some(state) = auth_state {
        routes.layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::cookie_auth_middleware,
        ))
    } else {
        routes
            .layer(Extension(AuthToken(String::new())))
            .layer(Extension(ValidatedUser {
                subject: "anonymous".to_string(),
                email: None,
                issuer: "local".to_string(),
                is_admin: true,
            }))
    }
}

fn build_protected_maps_blob_route(
    base_path: &str,
    auth_state: &Option<AuthState>,
    maps_state: maps::MapsState,
) -> Router {
    let routes = Router::new()
        .route(
            &format!("{base_path}/api/maps/blob/{{filename}}"),
            get(maps::maps_blob),
        )
        .layer(Extension(maps_state))
        .layer(middleware::from_fn(observability_middleware));

    if let Some(state) = auth_state {
        routes.layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::cookie_auth_middleware,
        ))
    } else {
        routes
            .layer(Extension(AuthToken(String::new())))
            .layer(Extension(ValidatedUser {
                subject: "anonymous".to_string(),
                email: None,
                issuer: "local".to_string(),
                is_admin: true,
            }))
    }
}

// ---------------------------------------------------------------------------
// Frontend / SPA
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct IndexState {
    frontend_dir: String,
    base_path: String,
}

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

    let base_href = if state.base_path.is_empty() {
        "/".to_string()
    } else {
        format!("{}/", state.base_path)
    };
    let injection = format!(
        r#"<base href="{base_href}"><script>window.__MICROMEGAS_CONFIG__={{basePath:"{}"}}</script>"#,
        state.base_path
    );

    let modified_html = html.replace("<head>", &format!("<head>{injection}"));

    Ok((
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        modified_html,
    ))
}

fn build_frontend(frontend_dir: &str, index_state: IndexState) -> Router {
    let spa_fallback = get(serve_index_with_config).with_state(index_state.clone());
    let serve_dir = ServeDir::new(frontend_dir).fallback(spa_fallback);

    let index_handler = get(serve_index_with_config).with_state(index_state);
    Router::new()
        .route("/", index_handler.clone())
        .route("/index.html", index_handler)
        .fallback_service(serve_dir)
}

fn mount_frontend(
    app: Router,
    base_path: &str,
    frontend: Router,
    index_state: IndexState,
) -> Router {
    if base_path.is_empty() {
        app.merge(frontend)
    } else {
        let app = app.nest(base_path, frontend);

        let base_path_with_slash = format!("{base_path}/");
        let index_handler = get(serve_index_with_config).with_state(index_state);
        let app = app.route(&base_path_with_slash, index_handler);

        let redirect_path = base_path.to_string();
        app.layer(axum::middleware::from_fn(
            move |req: axum::extract::Request, next: axum::middleware::Next| {
                let base_path = redirect_path.clone();
                async move {
                    if req.uri().path() == base_path {
                        return Redirect::permanent(&format!("{base_path}/")).into_response();
                    }
                    next.run(req).await.into_response()
                }
            },
        ))
    }
}

fn build_cors_layer(cors_origin: &str) -> Result<CorsLayer> {
    let origin = cors_origin
        .parse::<HeaderValue>()
        .context("Invalid MICROMEGAS_WEB_CORS_ORIGIN format")?;

    Ok(CorsLayer::new()
        .allow_origin(origin)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
        .allow_credentials(true))
}

// ---------------------------------------------------------------------------
// Simple handlers
// ---------------------------------------------------------------------------

async fn ready_check(Extension(state): Extension<Arc<ReadinessState>>) -> StatusCode {
    if state.check_ready().await {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

#[derive(Debug, Serialize)]
struct HealthCheck {
    status: String,
    timestamp: DateTime<Utc>,
    flightsql_connected: bool,
}

#[span_fn]
async fn health_check() -> impl IntoResponse {
    Json(HealthCheck {
        status: "healthy".to_string(),
        timestamp: Utc::now(),
        flightsql_connected: false,
    })
}

#[derive(Debug, Serialize)]
struct NoAuthUserInfo {
    sub: String,
    email: Option<String>,
    name: Option<String>,
    is_admin: bool,
}

async fn auth_me_no_auth() -> impl IntoResponse {
    Json(NoAuthUserInfo {
        sub: "anonymous".to_string(),
        email: Some("anonymous@localhost".to_string()),
        name: Some("Anonymous (No Auth)".to_string()),
        is_admin: true,
    })
}

async fn auth_logout_no_auth() -> impl IntoResponse {
    StatusCode::OK
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Start the analytics web server.
///
/// Binds to `config.port`, wires all routes, and shuts down gracefully when
/// `shutdown` resolves.  This is called by both the standalone binary and the
/// monolith.
pub async fn run_web_server(
    config: WebServerConfig,
    shutdown: impl Future<Output = ()> + Send + 'static,
    grace: Duration,
) -> Result<()> {
    let app_db_pool = sqlx::PgPool::connect(&config.app_db_string)
        .await
        .context("Failed to connect to micromegas_app database")?;
    app_db::execute_migration(app_db_pool.clone()).await?;
    info!("Connected to micromegas_app database");

    let data_source_cache =
        DataSourceCache::new(app_db_pool.clone(), std::time::Duration::from_secs(60));

    let maps_store = maps::connect_maps_store(config.maps_uri.as_deref())
        .context("Failed to connect to maps object store")?;
    if maps_store.is_some() {
        info!("Maps object store connected");
    } else {
        info!("MICROMEGAS_MAPS_OBJECT_STORE_URI not set — /api/maps/* will return 503");
    }
    let max_upload_bytes = config
        .max_upload_bytes
        .unwrap_or(maps::DEFAULT_MAX_UPLOAD_BYTES);
    let maps_state = maps::MapsState::with_max_upload_bytes(maps_store, max_upload_bytes);

    let auth_state = if config.disable_auth {
        println!("WARNING: Authentication is disabled (--disable-auth)");
        None
    } else {
        build_auth_state(&config)?
    };

    let readiness_state = Arc::new(ReadinessState::new(app_db_pool.clone()));

    let app = Router::new()
        .merge(build_public_routes(&config.base_path, readiness_state))
        .merge(build_protected_routes(
            &config.base_path,
            &auth_state,
            app_db_pool,
            data_source_cache,
            maps_state.clone(),
        ))
        .merge(build_auth_routes(&config.base_path, &auth_state));

    let index_state = IndexState {
        frontend_dir: config.frontend_dir.clone(),
        base_path: config.base_path.clone(),
    };
    let frontend = build_frontend(&config.frontend_dir, index_state.clone());
    let app = mount_frontend(app, &config.base_path, frontend, index_state);

    let app = app.layer(CompressionLayer::new().gzip(true));
    let app = app
        .merge(build_protected_maps_blob_route(
            &config.base_path,
            &auth_state,
            maps_state,
        ))
        .layer(build_cors_layer(&config.cors_origin)?);

    let addr = format!("0.0.0.0:{}", config.port);
    println!("Analytics web server starting on {addr}");
    println!("CORS origin: {}", config.cors_origin);
    if !config.base_path.is_empty() {
        println!("Base path: {}", config.base_path);
    }
    println!(
        "Authentication: {}",
        if config.disable_auth {
            "DISABLED"
        } else {
            "ENABLED"
        }
    );

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    serve_axum_with_graceful_shutdown(
        listener,
        ServiceExt::<axum::extract::Request>::into_make_service_with_connect_info::<
            std::net::SocketAddr,
        >(app),
        shutdown,
        grace,
    )
    .await?;

    Ok(())
}
