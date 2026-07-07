#[cfg(not(target_os = "windows"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use anyhow::{Context, Result, anyhow};
use axum::{
    Router,
    http::StatusCode,
    middleware,
    routing::{get, post},
};
use clap::Parser;
use micromegas::micromegas_main;
use micromegas::servers::shutdown::{serve_axum_with_graceful_shutdown, wait_for_sigterm};
use micromegas_auth::api_key::{ApiKeyAuthProvider, parse_key_ring};
use micromegas_auth::axum::auth_middleware;
use micromegas_auth::types::AuthProvider;
use micromegas_object_cache::foyer_backend::{FoyerBackend, WriteTuning};
use micromegas_object_cache::range_cache::RangeCache;
use micromegas_object_cache_srv::app_state::AppState;
use micromegas_object_cache_srv::cli::{self, Cli};
use micromegas_object_cache_srv::handlers::{
    get_range_handler, head_handler, permits_for_bytes, post_ranges_handler, prefetch_handler,
    stream_window_bytes,
};
use micromegas_object_cache_srv::prefetch_queue::spawn_prefetch_worker;
use micromegas_tracing::prelude::*;
use object_store::parse_url_opts;
use object_store::prefix::PrefixStore;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

#[micromegas_main(interop_max_level = "info")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();

    // `block_size` is the divisor for block-index math (`start / block_size`);
    // a value of 0 would panic on the first range read. Reject it at the startup
    // boundary as a fatal config error rather than letting it reach the cache.
    if args.block_size == 0 {
        return Err(anyhow!("MICROMEGAS_OBJECT_CACHE_BLOCK_SIZE must be greater than 0").into());
    }

    if args.max_concurrent_fetches == 0 {
        return Err(anyhow!(
            "MICROMEGAS_OBJECT_CACHE_MAX_CONCURRENT_FETCHES must be greater than 0"
        )
        .into());
    }
    // `FetchScheduler` computes `total - demand_reserved` as a plain
    // subtraction; a misconfigured pair would panic deep inside the cache
    // instead of at startup.
    if args.demand_reserved_fetches >= args.max_concurrent_fetches {
        return Err(anyhow!(
            "MICROMEGAS_OBJECT_CACHE_DEMAND_RESERVED_FETCHES ({}) must be less than \
             MICROMEGAS_OBJECT_CACHE_MAX_CONCURRENT_FETCHES ({})",
            args.demand_reserved_fetches,
            args.max_concurrent_fetches
        )
        .into());
    }
    // A zero budget would make every non-empty data request hang forever
    // acquiring its mem_permits charge while /health and /ready still pass;
    // fail at startup instead.
    if args.memory_budget_mb == 0 {
        return Err(
            anyhow!("MICROMEGAS_OBJECT_CACHE_MEMORY_BUDGET_MB must be greater than 0").into(),
        );
    }
    // A large streaming read still charges a full window's worth of permits
    // (`stream_window_bytes`, capped rather than rejected outright — see
    // `handlers::stream_window_bytes`), and `Semaphore::acquire_many_owned`
    // never completes (and never errors) if the requested count exceeds the
    // semaphore's total permits. Without this floor, a deployment configured
    // with a smaller `--memory-budget-mb` would hang every large read
    // instead of failing fast here at startup.
    let window_mb = permits_for_bytes(stream_window_bytes(args.block_size));
    if args.memory_budget_mb < window_mb {
        return Err(anyhow!(
            "MICROMEGAS_OBJECT_CACHE_MEMORY_BUDGET_MB ({}) must be at least {window_mb} MiB \
             (2 * DEMAND_WINDOW_BLOCKS * block_size, the largest charge a single streaming \
             request can make), or every large read would hang acquiring mem_permits",
            args.memory_budget_mb
        )
        .into());
    }
    if args.prefetch_queue_capacity == 0 {
        return Err(anyhow!(
            "MICROMEGAS_OBJECT_CACHE_PREFETCH_QUEUE_CAPACITY must be greater than 0"
        )
        .into());
    }
    if args.prefetch_worker_concurrency == 0 {
        return Err(anyhow!(
            "MICROMEGAS_OBJECT_CACHE_PREFETCH_WORKER_CONCURRENCY must be greater than 0"
        )
        .into());
    }
    cli::validate_write_tuning(args.flushers, args.write_buffer_mb)?;

    let ns = if args.namespace.is_empty() {
        // Strip any `scheme://` prefix so the namespace is stable regardless of
        // the origin scheme (s3://, gs://, file://, ...).
        args.origin_uri
            .split_once("://")
            .map_or(args.origin_uri.as_str(), |(_scheme, rest)| rest)
            .replace('/', "_")
    } else {
        args.namespace.clone()
    };

    let (origin_store, prefix) = parse_url_opts(
        &url::Url::parse(&args.origin_uri).with_context(|| "parsing origin URI")?,
        std::env::vars().map(|(k, v)| (k.to_lowercase(), v)),
    )
    .with_context(|| "building origin object store")?;
    // ORIGIN_URI must be bucket-only (no path component): the client wraps the
    // cache layer INSIDE its PrefixStore, so every request key already carries the
    // lake-root prefix (e.g. lakeroot/blocks/xyz). If ORIGIN_URI also carried that
    // path, the server's PrefixStore would prepend it AGAIN (lakeroot/lakeroot/...),
    // producing silent 404s. We therefore require the parsed prefix to be empty.
    if !prefix.as_ref().is_empty() {
        return Err(anyhow!(
            "ORIGIN_URI must be bucket-only with no path component (got prefix {:?}); \
             the lake-root prefix arrives inside each request key, so a path here \
             would be applied twice",
            prefix.as_ref()
        )
        .into());
    }
    let origin_store: Arc<dyn object_store::ObjectStore> =
        Arc::new(PrefixStore::new(origin_store, prefix));

    // The submit-queue overflow threshold is pinned to 2x the buffer pool
    // (foyer's documented -- but no longer automatic, see `WriteTuning` --
    // intended default); see `MICROMEGAS_OBJECT_CACHE_WRITE_BUFFER_MB`.
    let write_tuning = WriteTuning {
        flushers: args.flushers,
        buffer_pool_bytes: args.write_buffer_mb * 1024 * 1024,
        submit_queue_threshold_bytes: args.write_buffer_mb * 1024 * 1024 * 2,
    };
    let foyer = FoyerBackend::new_with_shards(
        &args.disk_path,
        args.ram_mb * 1024 * 1024,
        args.disk_gb * 1024 * 1024 * 1024,
        8,
        write_tuning,
    )
    .await
    .with_context(|| "building FoyerBackend")?;

    let cache = RangeCache::new(
        origin_store,
        Arc::new(foyer),
        args.block_size,
        ns,
        args.max_concurrent_fetches,
        args.demand_reserved_fetches,
        args.max_coalesced_get_bytes,
        args.promote_whole_batch,
    );

    // Resolve the prefix allowlist. Fail-closed like auth: an empty list is a
    // fatal config error unless `--allow-all-prefixes` is given (dev opt-out).
    // Reject blank entries (e.g. a trailing comma in the env var) — a blank
    // prefix would admit every key and silently defeat containment.
    let allowed_prefixes = if args.allow_all_prefixes {
        warn!("Object cache: serving ALL prefixes (--allow-all-prefixes)");
        Vec::new()
    } else {
        let prefixes: Vec<String> = args
            .allowed_prefixes
            .iter()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        if prefixes.is_empty() {
            return Err(
                "No allowed prefixes configured. Set MICROMEGAS_OBJECT_CACHE_PREFIX \
                 (comma-separated, e.g. `blobs,views`) or pass --prefix, \
                 or use --allow-all-prefixes for development"
                    .into(),
            );
        }
        info!("Object cache: allowed prefixes {prefixes:?}");
        prefixes
    };

    // Attach the `prefix` metric-tag classifier now that `allowed_prefixes`
    // is resolved: labels are leaked to `'static` once here (bounded,
    // low-cardinality, set once at startup) so the hot per-block emission
    // sites in `fetch_blocks` can hold `&'static PropertySet`s without an
    // allocation per call. `--allow-all-prefixes` leaves this list empty, so
    // every key classifies as `"other"` (`RangeCache::new`'s own default).
    let prefix_labels: Arc<[&'static str]> = allowed_prefixes
        .iter()
        .map(|p| -> &'static str { Box::leak(p.clone().into_boxed_str()) })
        .collect::<Vec<_>>()
        .into();
    let cache = cache.with_prefix_labels(prefix_labels);

    let (prefetch_tx, _prefetch_worker) = spawn_prefetch_worker(
        cache.clone(),
        args.prefetch_queue_capacity,
        args.prefetch_worker_concurrency,
    );

    let state = AppState::new(cache, allowed_prefixes, args.memory_budget_mb, prefetch_tx);

    // Periodic saturation gauges (fetch-budget occupancy, in-flight entries,
    // memory-budget occupancy, prefetch queue depth, host NIC throughput,
    // foyer disk write-path throughput) -- see `saturation_monitor` for what
    // each signals. Spawned detached: the sampler runs for the process
    // lifetime, like the prefetch worker above.
    let _saturation_monitor =
        micromegas_object_cache_srv::saturation_monitor::spawn_saturation_monitor(
            state.cache.clone(),
            state.mem_permits.clone(),
            state.memory_budget_mb,
            state.prefetch_tx.clone(),
        );

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
        .route("/ranges/{*key}", post(post_ranges_handler))
        .route("/prefetch", post(prefetch_handler))
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
