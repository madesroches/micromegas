use anyhow::{Context, Result};
use micromegas_object_cache::CacheClientStore;
use micromegas_object_cache::prefetch::{ObjectPrefetch, PrefetchItem, PrefixPrefetch};
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_tracing::prelude::*;
use object_store::ObjectStore;
use sqlx::PgPool;
use sqlx::postgres::{PgConnection, PgPoolOptions};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

/// A connection to the data lake, including a database pool and a blob storage client.
#[derive(Debug, Clone)]
pub struct DataLakeConnection {
    pub db_pool: PgPool,
    pub blob_storage: Arc<BlobStorage>,
    /// `Some` when the object cache is configured for this connection; used to
    /// fire-and-forget warm freshly-written objects (`warm_object`).
    /// `None` when the cache is not configured.
    prefetch: Option<Arc<dyn ObjectPrefetch>>,
}

impl DataLakeConnection {
    pub fn new(db_pool: PgPool, blob_storage: Arc<BlobStorage>) -> Self {
        Self {
            db_pool,
            blob_storage,
            prefetch: None,
        }
    }

    /// Like `new`, but also wires the object cache's prefetch face for
    /// write-time warming (see `warm_object`).
    pub fn new_with_prefetch(
        db_pool: PgPool,
        blob_storage: Arc<BlobStorage>,
        prefetch: Option<Arc<dyn ObjectPrefetch>>,
    ) -> Self {
        Self {
            db_pool,
            blob_storage,
            prefetch,
        }
    }

    /// Warm a freshly-written object in the object cache by key. Fire-and-forget
    /// at prefetch priority: spawns a detached task and returns immediately, so the
    /// caller's write path is never delayed or failed by a warm. No-op when the
    /// cache is not configured or `size <= 0`. Returns the spawned task handle (or
    /// None) purely so tests can await completion deterministically; production
    /// callers ignore it.
    ///
    /// This is a general "warm any object" primitive — the write-partition path is
    /// its first caller, but nothing here is partition-specific (e.g. the ingestion
    /// service could warm raw payloads the same way). `key` is the lake-root-relative
    /// object key; the configured prefetch handle applies the lake root prefix so the
    /// warmed key matches the key demand reads produce.
    pub fn warm_object(&self, key: &str, size: i64) -> Option<JoinHandle<()>> {
        let prefetch = self.prefetch.as_ref()?.clone();
        if size <= 0 {
            return None; // nothing to warm
        }
        let key = key.to_string(); // owned copy: the spawned future must be 'static
        let item = PrefetchItem {
            key: key.clone(),
            size: size as u64,
            ranges: None,
        };
        imetric!("object_warm_requested", "count", 1_u64);
        Some(spawn_with_context(async move {
            match prefetch.prefetch(vec![item]).await {
                Ok(resp) => debug!(
                    "write-time warm enqueued accepted={} rejected={} dropped={}",
                    resp.accepted, resp.rejected, resp.dropped
                ),
                // CacheClientStore::prefetch already bumps range_cache_client_prefetch_error;
                // keep this at debug — a failed warm just means the first read is a cold miss.
                Err(e) => debug!("write-time warm failed for {key}: {e}"),
            }
        }))
    }
}

/// Wrap `direct` with the object cache when configured, returning the store
/// layer and — when enabled — the same client's `ObjectPrefetch` face for
/// write-time warming.
pub(crate) fn make_cache(
    direct: Arc<dyn ObjectStore>,
) -> (Arc<dyn ObjectStore>, Option<Arc<dyn ObjectPrefetch>>) {
    let cache_url = std::env::var("MICROMEGAS_OBJECT_CACHE_URL").ok();
    let api_key = std::env::var("MICROMEGAS_OBJECT_CACHE_API_KEY").ok();
    match cache_url {
        Some(url) if api_key.is_some() => {
            let client = Arc::new(CacheClientStore::new(url, api_key, direct));
            (
                client.clone() as Arc<dyn ObjectStore>,
                Some(client as Arc<dyn ObjectPrefetch>),
            )
        }
        Some(url) => {
            // URL without key: disabled, warn (preserve current behavior)
            warn!(
                "MICROMEGAS_OBJECT_CACHE_URL is set ({url}) but MICROMEGAS_OBJECT_CACHE_API_KEY is missing: the object cache is disabled and requests will go directly to the store"
            );
            (direct, None)
        }
        None => (direct, None),
    }
}

/// Which write-session guarantee a lake connection pool enforces on every connection it hands
/// out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritablePolicy {
    /// Never hand out a read-only connection: reject it in `after_connect` (sqlx retries with
    /// a fresh DNS lookup) and evict it in `before_acquire` if it turned read-only while pooled.
    /// A write that arrives during a failover waits in `acquire` instead of failing against a
    /// demoted writer.
    Require,
    /// Keep trying for a writable connection but fall back to a read-only one after
    /// [`FALLBACK_AFTER`] with no writable connect, re-probing for a writable primary every
    /// [`PROBE_INTERVAL`] while on a replica. Used by standalone FlightSQL, which has no other
    /// writer to fail over to and would rather serve reads from a replica than refuse to start.
    Prefer,
}

/// How long a [`WritablePolicy::Prefer`] pool keeps rejecting read-only connections before
/// falling back, once no writable connection has succeeded since the last one (or since
/// startup).
pub const FALLBACK_AFTER: Duration = Duration::from_secs(10);

/// How often a [`WritablePolicy::Prefer`] pool that has fallen back to a read-only connection
/// evicts a pooled one to re-probe for a writable primary.
pub const PROBE_INTERVAL: Duration = Duration::from_secs(30);

/// True when `setting` (a Postgres `transaction_read_only` value, as read by `SHOW
/// transaction_read_only`) marks the session as read-only. Anything other than `"on"` counts as
/// writable: this matches libpq's `target_session_attrs=read-write` on pre-14 servers, and an
/// unexpected value shouldn't take the service down.
pub fn is_read_only(setting: &str) -> bool {
    setting == "on"
}

async fn connection_is_read_only(conn: &mut PgConnection) -> Result<bool, sqlx::Error> {
    let setting: String = sqlx::query_scalar("SHOW transaction_read_only")
        .fetch_one(conn)
        .await?;
    Ok(is_read_only(&setting))
}

fn read_only_rejection() -> sqlx::Error {
    sqlx::Error::Io(io::Error::other(
        "connected to a read-only postgres instance",
    ))
}

/// Pool-wide state backing [`WritablePolicy::Prefer`]'s accept/evict decisions, shared by every
/// connection of the pool (and by every pool cloned from its options) via the `Arc` the
/// `after_connect`/`before_acquire` closures capture. Never held across an await.
#[derive(Debug)]
struct ReadOnlyFallbackState {
    /// The first read-only rejection since the last writable connect (`None` once a writable
    /// connect has cleared it).
    streak_start: Option<Instant>,
    /// The last time a pooled read-only connection was evicted for a re-probe, `None` until the
    /// first re-probe of the current streak.
    last_probe: Option<Instant>,
}

/// Shared handle to a [`WritablePolicy::Prefer`] pool's fallback state. See [`Self::on_connect`]
/// and [`Self::on_acquire`] for the decisions it drives.
#[derive(Debug, Clone)]
pub struct ReadOnlyFallback(Arc<Mutex<ReadOnlyFallbackState>>);

impl Default for ReadOnlyFallback {
    fn default() -> Self {
        Self::new()
    }
}

/// Clears an in-progress fallback streak on a writable connect/acquire (shared by
/// [`ReadOnlyFallback::on_connect`] and [`ReadOnlyFallback::on_acquire`]).
fn clear_streak(state: &mut ReadOnlyFallbackState) {
    if state.streak_start.take().is_some() {
        info!("writable postgres connection succeeded, ending read-only fallback");
    }
    state.last_probe = None;
}

impl ReadOnlyFallback {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(ReadOnlyFallbackState {
            streak_start: None,
            last_probe: None,
        })))
    }

    /// `after_connect` decision for a freshly opened connection: accept it? A writable
    /// connection always clears the streak and is accepted. A read-only connection starts the
    /// streak (if none is running) and is rejected until `now - streak_start >= FALLBACK_AFTER`.
    pub fn on_connect(&self, read_only: bool, now: Instant) -> bool {
        let mut state = self.0.lock().expect("ReadOnlyFallback mutex poisoned");
        if !read_only {
            clear_streak(&mut state);
            return true;
        }
        let streak_start = *state.streak_start.get_or_insert(now);
        now.duration_since(streak_start) >= FALLBACK_AFTER
    }

    /// `before_acquire` decision for a pooled connection: keep it? A writable connection clears
    /// the streak (the same as [`Self::on_connect`]) and is kept. A read-only connection is
    /// evicted immediately if no fallback streak is running (a writable connect has succeeded
    /// since the fallback, so this connection is stale) or if the streak hasn't reached
    /// `FALLBACK_AFTER` yet (a connection turned read-only mid-pool, but we haven't actually
    /// fallen back -- `on_connect`'s window check doesn't cover already-pooled connections, so
    /// this is the only place that can reject them). Once the window has elapsed, it's evicted
    /// at most once per `PROBE_INTERVAL`, as a re-probe, and kept in between.
    pub fn on_acquire(&self, read_only: bool, now: Instant) -> bool {
        let mut state = self.0.lock().expect("ReadOnlyFallback mutex poisoned");
        if !read_only {
            clear_streak(&mut state);
            return true;
        }
        let Some(streak_start) = state.streak_start else {
            return false;
        };
        if now.duration_since(streak_start) < FALLBACK_AFTER {
            return false;
        }
        match state.last_probe {
            Some(probed_at) if now.duration_since(probed_at) < PROBE_INTERVAL => true,
            _ => {
                state.last_probe = Some(now);
                false
            }
        }
    }
}

/// Pool options for a pool whose write-session guarantee is `policy` -- see [`WritablePolicy`].
///
/// Both variants install `after_connect`/`before_acquire` hooks that run `SHOW
/// transaction_read_only` and `test_before_acquire(false)`: the `before_acquire` query already
/// makes one round trip and fails on a dead connection, so it replaces sqlx's default `ping`,
/// keeping the hot path at one round trip per acquire.
pub fn pool_options(policy: WritablePolicy) -> PgPoolOptions {
    let options = PgPoolOptions::new().test_before_acquire(false);
    match policy {
        WritablePolicy::Require => options
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    if connection_is_read_only(conn).await? {
                        warn!("connected to a read-only postgres instance, reconnecting");
                        imetric!("pg_read_only_connection_rejected", "count", 1_u64);
                        Err(read_only_rejection())
                    } else {
                        Ok(())
                    }
                })
            })
            .before_acquire(|conn, _meta| {
                Box::pin(async move {
                    if connection_is_read_only(conn).await? {
                        warn!("evicting pooled connection that turned read-only");
                        imetric!("pg_read_only_connection_rejected", "count", 1_u64);
                        Ok(false)
                    } else {
                        Ok(true)
                    }
                })
            }),
        WritablePolicy::Prefer => {
            let fallback = ReadOnlyFallback::new();
            let connect_fallback = fallback.clone();
            let acquire_fallback = fallback;
            options
                .after_connect(move |conn, _meta| {
                    let fallback = connect_fallback.clone();
                    Box::pin(async move {
                        let read_only = connection_is_read_only(conn).await?;
                        if !fallback.on_connect(read_only, Instant::now()) {
                            warn!("connected to a read-only postgres instance, reconnecting");
                            imetric!("pg_read_only_connection_rejected", "count", 1_u64);
                            return Err(read_only_rejection());
                        }
                        if read_only {
                            warn!(
                                "accepting a read-only postgres connection: no writable primary within the fallback window"
                            );
                            imetric!("pg_read_only_connection_accepted", "count", 1_u64);
                        }
                        Ok(())
                    })
                })
                .before_acquire(move |conn, _meta| {
                    let fallback = acquire_fallback.clone();
                    Box::pin(async move {
                        let read_only = connection_is_read_only(conn).await?;
                        if fallback.on_acquire(read_only, Instant::now()) {
                            Ok(true)
                        } else {
                            warn!("evicting pooled read-only postgres connection");
                            imetric!("pg_read_only_connection_rejected", "count", 1_u64);
                            Ok(false)
                        }
                    })
                })
        }
    }
}

/// Pool options for a pool that must reach a writable primary -- [`pool_options`] with
/// [`WritablePolicy::Require`].
pub fn read_write_pool_options() -> PgPoolOptions {
    pool_options(WritablePolicy::Require)
}

/// Finds a `sqlx::Error` carrying SQLSTATE `25006` (`cannot execute ... in a read-only
/// transaction`) anywhere in `err`'s chain -- the error a write raises when its connection turned
/// read-only mid-query, after already passing this crate's connect/acquire checks (e.g. an
/// instance demoted between `before_acquire` and the query, or a [`WritablePolicy::Prefer`] pool
/// that fell back to a replica on purpose).
pub fn is_read_only_violation(err: &anyhow::Error) -> bool {
    err.chain()
        .find_map(|e| e.downcast_ref::<sqlx::Error>())
        .and_then(sqlx::Error::as_database_error)
        .and_then(|db_err| db_err.code())
        .is_some_and(|code| code == "25006")
}

/// Connects to the data lake.
pub async fn connect_to_data_lake(
    policy: WritablePolicy,
    db_uri: &str,
    object_store_url: &str,
) -> Result<DataLakeConnection> {
    info!("connecting to blob storage");
    let (raw_store, root) = BlobStorage::parse_url_opts(object_store_url)
        .with_context(|| "connecting to blob storage")?;
    let (layered, prefetch_client) = make_cache(raw_store);
    let blob_storage = Arc::new(BlobStorage::new(layered, root.clone()));
    let prefetch =
        prefetch_client.map(|p| Arc::new(PrefixPrefetch::new(p, root)) as Arc<dyn ObjectPrefetch>);
    let pool = pool_options(policy)
        .connect(db_uri)
        .await
        .with_context(|| String::from("Connecting to telemetry database"))?;
    Ok(DataLakeConnection::new_with_prefetch(
        pool,
        blob_storage,
        prefetch,
    ))
}
