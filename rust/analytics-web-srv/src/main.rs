mod data_sources;
mod screens;

use analytics_web_srv::app_db;
use analytics_web_srv::auth;
use analytics_web_srv::data_source_cache::DataSourceCache;
use analytics_web_srv::stream_query;
use anyhow::{Context, Result};
use auth::{AuthState, AuthToken, OidcClientConfig};
use axum::{
    Extension, Json, Router, ServiceExt,
    extract::State,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Redirect},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use clap::Parser;
use http::{HeaderValue, Method, header};
use micromegas::micromegas_main;
use micromegas::servers::axum_utils::observability_middleware;
use micromegas::tracing::prelude::*;
#[allow(unused_imports)]
use micromegas_auth::{axum::auth_middleware, types::AuthProvider};
use serde::Serialize;
use sqlx::PgPool;
use std::sync::Arc;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, services::ServeDir};

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

#[derive(Debug, Serialize)]
struct HealthCheck {
    status: String,
    timestamp: DateTime<Utc>,
    flightsql_connected: bool,
}

/// State for serving index.html with injected runtime config
#[derive(Clone)]
struct IndexState {
    frontend_dir: String,
    base_path: String,
}

// ---------------------------------------------------------------------------
// Auth setup
// ---------------------------------------------------------------------------

fn build_auth_state(base_path: &str) -> Result<Option<AuthState>> {
    let oidc_config = OidcClientConfig::from_env()
        .map_err(|e| anyhow::anyhow!("Failed to load OIDC client config: {e}"))?;

    let cookie_domain = std::env::var("MICROMEGAS_COOKIE_DOMAIN").ok();
    let secure_cookies = std::env::var("MICROMEGAS_SECURE_COOKIES")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    // HMAC-SHA256 secret for signing OAuth state parameters (CSRF protection).
    // Must be identical across all instances in a scaled deployment.
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
        base_path: base_path.to_string(),
    }))
}

fn build_auth_routes(base_path: &str, auth_state: &Option<AuthState>) -> Router {
    if let Some(state) = auth_state {
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
// API routes
// ---------------------------------------------------------------------------

fn build_public_routes(base_path: &str) -> Router {
    Router::new().route(&format!("{base_path}/api/health"), get(health_check))
}

/// All routes that require a valid session (or dummy extensions in no-auth mode).
fn build_protected_routes(
    base_path: &str,
    auth_state: &Option<AuthState>,
    app_db_pool: PgPool,
    data_source_cache: DataSourceCache,
) -> Router {
    let routes = Router::new()
        // Query streaming
        .route(
            &format!("{base_path}/api/query-stream"),
            post(stream_query::stream_query_handler),
        )
        // Screen types (static)
        .route(
            &format!("{base_path}/api/screen-types"),
            get(screens::list_screen_types),
        )
        .route(
            &format!("{base_path}/api/screen-types/{{type_name}}/default"),
            get(screens::get_default_config),
        )
        // Screens CRUD
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
        // Data sources CRUD
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
        .layer(Extension(app_db_pool))
        .layer(Extension(data_source_cache))
        .layer(middleware::from_fn(observability_middleware));

    if let Some(state) = auth_state {
        routes.layer(middleware::from_fn_with_state(
            state.clone(),
            auth::cookie_auth_middleware,
        ))
    } else {
        routes
            .layer(Extension(AuthToken(String::new())))
            .layer(Extension(auth::ValidatedUser {
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

/// Serve index.html with runtime config injected into `<head>`.
///
/// The `<base>` tag ensures relative asset URLs resolve correctly from any
/// URL path, and the config script provides the base path for API calls.
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
// Read base path from environment
// ---------------------------------------------------------------------------

fn read_base_path() -> Result<String> {
    let raw = std::env::var("MICROMEGAS_BASE_PATH")
        .context("MICROMEGAS_BASE_PATH environment variable not set")?;
    let base_path = raw.trim_end_matches('/').to_string();
    if !base_path.is_empty() && !base_path.starts_with('/') {
        anyhow::bail!("MICROMEGAS_BASE_PATH must start with '/' (e.g., '/', '/micromegas')");
    }
    Ok(base_path)
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[micromegas_main(interop_max_level = "info")]
async fn main() -> Result<()> {
    let args = Args::parse();
    let cors_origin = std::env::var("MICROMEGAS_WEB_CORS_ORIGIN")
        .context("MICROMEGAS_WEB_CORS_ORIGIN environment variable not set")?;
    let base_path = read_base_path()?;

    // Database
    let app_db_pool = sqlx::PgPool::connect(
        &std::env::var("MICROMEGAS_APP_SQL_CONNECTION_STRING")
            .context("MICROMEGAS_APP_SQL_CONNECTION_STRING environment variable not set")?,
    )
    .await
    .context("Failed to connect to micromegas_app database")?;
    app_db::execute_migration(app_db_pool.clone()).await?;
    info!("Connected to micromegas_app database");

    let data_source_cache =
        DataSourceCache::new(app_db_pool.clone(), std::time::Duration::from_secs(60));

    // Auth
    let auth_state = if args.disable_auth {
        println!("WARNING: Authentication is disabled (--disable-auth)");
        None
    } else {
        build_auth_state(&base_path)?
    };

    // Routes
    let app = Router::new()
        .merge(build_public_routes(&base_path))
        .merge(build_protected_routes(
            &base_path,
            &auth_state,
            app_db_pool,
            data_source_cache,
        ))
        .merge(build_auth_routes(&base_path, &auth_state));

    // Frontend
    let index_state = IndexState {
        frontend_dir: args.frontend_dir.clone(),
        base_path: base_path.clone(),
    };
    let frontend = build_frontend(&args.frontend_dir, index_state.clone());
    let app = mount_frontend(app, &base_path, frontend, index_state);

    // Global middleware
    let app = app
        .layer(CompressionLayer::new().gzip(true))
        .layer(build_cors_layer(&cors_origin)?);

    // Start server
    let addr = format!("0.0.0.0:{}", args.port);
    println!("Analytics web server starting on {addr}");
    println!("CORS origin: {cors_origin}");
    if !base_path.is_empty() {
        println!("Base path: {base_path}");
    }
    println!(
        "Authentication: {}",
        if args.disable_auth {
            "DISABLED"
        } else {
            "ENABLED"
        }
    );

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

// ---------------------------------------------------------------------------
// Simple handlers
// ---------------------------------------------------------------------------

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
