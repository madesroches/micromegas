mod data_sources;
mod screens;

use analytics_web_srv::app_db;
use analytics_web_srv::auth;
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

    // Build the base tag and config script to inject
    // The <base> tag ensures relative asset URLs (./assets/...) resolve correctly
    // from any URL path, not just the root. Without this, accessing /base/screen/foo
    // would try to load ./assets/x.js from /base/screen/assets/x.js instead of /base/assets/x.js
    let base_href = if state.base_path.is_empty() {
        "/".to_string()
    } else {
        format!("{}/", state.base_path)
    };
    let injection = format!(
        r#"<base href="{base_href}"><script>window.__MICROMEGAS_CONFIG__={{basePath:"{}"}}</script>"#,
        state.base_path
    );

    // Inject right after <head> - the base tag MUST come before any relative URLs
    // to take effect (per HTML spec). Injecting before </head> is too late since
    // the script/link tags with relative paths appear before that point.
    let modified_html = html.replace("<head>", &format!("<head>{injection}"));

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
    let app_db_conn_string = std::env::var("MICROMEGAS_APP_SQL_CONNECTION_STRING")
        .context("MICROMEGAS_APP_SQL_CONNECTION_STRING environment variable not set")?;

    let app_db_pool = sqlx::PgPool::connect(&app_db_conn_string)
        .await
        .context("Failed to connect to micromegas_app database")?;

    app_db::execute_migration(app_db_pool.clone())
        .await
        .context("Failed to run micromegas_app migrations")?;

    info!("Connected to micromegas_app database");

    // Create data source cache with 60-second TTL
    let data_source_cache = analytics_web_srv::data_source_cache::DataSourceCache::new(
        app_db_pool.clone(),
        std::time::Duration::from_secs(60),
    );

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

    let health_routes = Router::new().route(&format!("{base_path}/api/health"), get(health_check));

    let api_routes = Router::new()
        .route(
            &format!("{base_path}/api/query-stream"),
            post(stream_query::stream_query_handler),
        )
        .layer(Extension(data_source_cache.clone()))
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

    // Build app API routes (screens + data sources)
    let app_api_routes = Router::new()
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
        .layer(Extension(data_source_cache.clone()))
        .layer(middleware::from_fn(observability_middleware));

    // Apply auth middleware if enabled, otherwise inject a no-auth dummy user
    let app_api_routes = if let Some(auth_state) = auth_state.clone() {
        app_api_routes.layer(middleware::from_fn_with_state(
            auth_state,
            auth::cookie_auth_middleware,
        ))
    } else {
        app_api_routes.layer(Extension(auth::ValidatedUser {
            subject: "anonymous".to_string(),
            email: None,
            issuer: "local".to_string(),
            is_admin: true,
        }))
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
        .merge(app_api_routes);

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

    // Add compression and CORS layers to the router
    let app = app
        .layer(CompressionLayer::new().gzip(true))
        .layer(cors_layer);

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

/// Stub /auth/logout endpoint for no-auth mode
async fn auth_logout_no_auth() -> impl IntoResponse {
    StatusCode::OK
}
