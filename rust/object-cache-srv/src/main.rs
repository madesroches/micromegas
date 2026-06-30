#[cfg(not(target_os = "windows"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use anyhow::{Context, Result, anyhow, bail};
use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    middleware,
    response::Response,
    routing::{get, post},
};
use bytes::{BufMut, Bytes, BytesMut};
use clap::Parser;
use micromegas::micromegas_main;
use micromegas::servers::shutdown::{serve_axum_with_graceful_shutdown, wait_for_sigterm};
use micromegas_auth::api_key::{ApiKeyAuthProvider, parse_key_ring};
use micromegas_auth::axum::auth_middleware;
use micromegas_auth::types::AuthProvider;
use micromegas_range_cache::foyer_backend::FoyerBackend;
use micromegas_range_cache::range_cache::{RangeCache, RangeError};
use micromegas_tracing::prelude::*;
use object_store::parse_url_opts;
use object_store::prefix::PrefixStore;
use serde::Deserialize;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser, Debug)]
#[clap(name = "micromegas-object-cache-srv")]
#[clap(about = "Shared object range cache service", version, author)]
struct Cli {
    #[clap(
        long,
        env = "MICROMEGAS_OBJECT_CACHE_LISTEN",
        default_value = "0.0.0.0:8080"
    )]
    listen: SocketAddr,

    #[clap(long, env = "MICROMEGAS_OBJECT_CACHE_ORIGIN_URI")]
    origin_uri: String,

    #[clap(long, env = "MICROMEGAS_OBJECT_CACHE_RAM_MB", default_value = "512")]
    ram_mb: usize,

    #[clap(long, env = "MICROMEGAS_OBJECT_CACHE_DISK_PATH")]
    disk_path: String,

    #[clap(long, env = "MICROMEGAS_OBJECT_CACHE_DISK_GB", default_value = "50")]
    disk_gb: usize,

    #[clap(
        long,
        env = "MICROMEGAS_OBJECT_CACHE_BLOCK_SIZE",
        default_value = "1048576"
    )]
    block_size: u64,

    #[clap(long, env = "MICROMEGAS_OBJECT_CACHE_NAMESPACE", default_value = "")]
    namespace: String,

    #[clap(long, env = "MICROMEGAS_OBJECT_CACHE_PREFIX", default_value = "")]
    allowed_prefix: String,

    #[clap(long, env = "MICROMEGAS_API_KEYS", default_value = "")]
    api_keys: String,

    /// Disable authentication (development mode only)
    #[clap(long)]
    disable_auth: bool,

    #[clap(
        long,
        default_value = "25",
        env = "MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS"
    )]
    shutdown_grace_period_seconds: u64,
}

#[derive(Clone)]
struct AppState {
    cache: RangeCache,
    allowed_prefix: String,
}

fn validate_key(key: &str, allowed_prefix: &str) -> Result<()> {
    if key.is_empty() {
        bail!("empty key");
    }
    if key.starts_with('/') {
        bail!("key must not start with /");
    }
    if key.split('/').any(|seg| seg == "..") {
        bail!("key must not contain ..");
    }
    if !allowed_prefix.is_empty() && !key.starts_with(allowed_prefix) {
        bail!("key {key} is outside allowed prefix {allowed_prefix}");
    }
    Ok(())
}

fn parse_range_header(header_value: &str, file_size: u64) -> Result<std::ops::Range<u64>> {
    let value = header_value
        .strip_prefix("bytes=")
        .ok_or_else(|| anyhow!("invalid Range header: {header_value}"))?;
    let (start_str, end_str) = value
        .split_once('-')
        .ok_or_else(|| anyhow!("invalid Range header format: {header_value}"))?;
    let start: u64 = start_str.parse().with_context(|| "parsing range start")?;
    let end: u64 = if end_str.is_empty() {
        file_size
    } else {
        end_str
            .parse::<u64>()
            .with_context(|| "parsing range end")?
            + 1
    };
    Ok(start..end)
}

fn is_not_found(e: &anyhow::Error) -> bool {
    if let Some(os_err) = e.downcast_ref::<object_store::Error>() {
        matches!(os_err, object_store::Error::NotFound { .. })
    } else {
        false
    }
}

async fn head_handler(
    Path(key): Path<String>,
    State(state): State<AppState>,
) -> Result<Response, StatusCode> {
    if let Err(e) = validate_key(&key, &state.allowed_prefix) {
        warn!("rejected key {key}: {e}");
        return Err(StatusCode::BAD_REQUEST);
    }
    match state.cache.size(&key).await {
        Ok(size) => {
            let response = Response::builder()
                .status(StatusCode::OK)
                .header("Content-Length", size.to_string())
                .body(Body::empty())
                .expect("build HEAD response");
            Ok(response)
        }
        Err(e) => {
            if is_not_found(&e) {
                Err(StatusCode::NOT_FOUND)
            } else {
                error!("HEAD {key}: {e:?}");
                Err(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    }
}

async fn get_range_handler(
    Path(key): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    if let Err(e) = validate_key(&key, &state.allowed_prefix) {
        warn!("rejected key {key}: {e}");
        return Err(StatusCode::BAD_REQUEST);
    }

    let file_size = match state.cache.size(&key).await {
        Ok(s) => s,
        Err(e) => {
            if is_not_found(&e) {
                return Err(StatusCode::NOT_FOUND);
            }
            error!("size {key}: {e:?}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let range_header = match headers.get("range").or_else(|| headers.get("Range")) {
        Some(h) => h.to_str().unwrap_or("").to_string(),
        None => format!("bytes=0-{}", file_size.saturating_sub(1)),
    };

    let byte_range = match parse_range_header(&range_header, file_size) {
        Ok(r) => r,
        Err(e) => {
            warn!("bad Range header {range_header}: {e}");
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    match state.cache.get_range(&key, byte_range.clone()).await {
        Ok(data) => {
            let content_length = data.len();
            let response = Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header("Content-Type", "application/octet-stream")
                .header("Content-Length", content_length.to_string())
                .header(
                    "Content-Range",
                    format!(
                        "bytes {}-{}/{}",
                        byte_range.start,
                        byte_range.end.saturating_sub(1),
                        file_size
                    ),
                )
                .body(Body::from(data))
                .expect("build GET response");
            Ok(response)
        }
        Err(e) => {
            if let Some(RangeError::OutOfBounds { .. }) = e.downcast_ref::<RangeError>() {
                warn!("range {byte_range:?} out of bounds for {key}: {e}");
                return Err(StatusCode::RANGE_NOT_SATISFIABLE);
            }
            error!("get_range {key} {byte_range:?}: {e:?}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(Deserialize)]
struct RangesRequest {
    ranges: Vec<[u64; 2]>,
}

async fn post_ranges_handler(
    Path(key): Path<String>,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Response, StatusCode> {
    if let Err(e) = validate_key(&key, &state.allowed_prefix) {
        warn!("rejected key {key}: {e}");
        return Err(StatusCode::BAD_REQUEST);
    }

    let req: RangesRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            warn!("bad ranges JSON: {e}");
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    let ranges: Vec<std::ops::Range<u64>> = req.ranges.iter().map(|&[s, e]| s..e).collect();

    match state.cache.get_ranges(&key, &ranges).await {
        Ok(results) => {
            let mut buf = BytesMut::new();
            for chunk in &results {
                buf.put_u64_le(chunk.len() as u64);
                buf.put_slice(chunk);
            }
            let response = Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/octet-stream")
                .body(Body::from(buf.freeze()))
                .expect("build ranges response");
            Ok(response)
        }
        Err(e) => {
            error!("get_ranges {key}: {e:?}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[micromegas_main(interop_max_level = "info")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();

    let ns = if args.namespace.is_empty() {
        args.origin_uri
            .trim_start_matches("s3://")
            .replace('/', "_")
    } else {
        args.namespace.clone()
    };

    let (origin_store, prefix) = parse_url_opts(
        &url::Url::parse(&args.origin_uri).with_context(|| "parsing origin URI")?,
        std::env::vars().map(|(k, v)| (k.to_lowercase(), v)),
    )
    .with_context(|| "building origin object store")?;
    // Re-apply the URI path prefix so an origin like s3://bucket/lakeroot resolves
    // keys under bucket/lakeroot/<key> instead of bucket/<key>.
    let origin_store: Arc<dyn object_store::ObjectStore> =
        Arc::new(PrefixStore::new(origin_store, prefix));

    let foyer = FoyerBackend::new(
        &args.disk_path,
        args.ram_mb * 1024 * 1024,
        args.disk_gb * 1024 * 1024 * 1024,
    )
    .await
    .with_context(|| "building FoyerBackend")?;

    let cache = RangeCache::new(origin_store, Arc::new(foyer), args.block_size, ns);

    let state = AppState {
        cache,
        allowed_prefix: args.allowed_prefix.clone(),
    };

    let auth_provider: Option<Arc<dyn AuthProvider>> = if args.disable_auth {
        info!("Authentication disabled (--disable-auth)");
        None
    } else if args.api_keys.is_empty() {
        return Err("Authentication required but no API keys configured. \
             Set MICROMEGAS_API_KEYS, or use --disable-auth for development"
            .into());
    } else {
        let keyring =
            parse_key_ring(&args.api_keys).with_context(|| "parsing MICROMEGAS_API_KEYS")?;
        Some(Arc::new(ApiKeyAuthProvider::new(keyring)))
    };

    let health_router = Router::new()
        .route("/health", get(|| async { StatusCode::OK }))
        .route("/ready", get(|| async { StatusCode::OK }));

    let obj_router = Router::new()
        .route("/obj/{*key}", get(get_range_handler).head(head_handler))
        .route("/obj/{*key}/ranges", post(post_ranges_handler))
        .with_state(state);

    let obj_router = if let Some(provider) = auth_provider {
        info!("Object cache: authentication enabled");
        obj_router.layer(middleware::from_fn(move |req, next| {
            auth_middleware(provider.clone(), req, next)
        }))
    } else {
        warn!("Object cache: authentication disabled");
        obj_router
    };

    let app = health_router.merge(obj_router);

    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("binding to {}", args.listen))?;

    info!("object-cache-srv listening on {}", args.listen);

    let grace = Duration::from_secs(args.shutdown_grace_period_seconds);
    serve_axum_with_graceful_shutdown(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
        wait_for_sigterm(),
        grace,
    )
    .await?;

    Ok(())
}
