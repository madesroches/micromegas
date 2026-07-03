use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};

use anyhow::{Result, anyhow};
use bytes::Bytes;
use futures::future::join_all;
use futures::stream::{self, FuturesUnordered, StreamExt};
use micromegas_tracing::prelude::*;
use object_store::{ObjectStore, ObjectStoreExt, path::Path};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, watch};

use super::backend::{FillHint, RangeCacheBackend};
use super::blocks::{assemble_range, block_byte_range, blocks_for_range, coalesce_runs};

/// Errors returned by [`RangeCache`] that callers may want to handle distinctly.
#[derive(Debug, thiserror::Error)]
pub enum RangeError {
    /// The requested range extends past the end of the object.
    #[error("requested range end {requested_end} exceeds object size {file_size}")]
    OutOfBounds { requested_end: u64, file_size: u64 },
}

pub const DEFAULT_BLOCK_SIZE: u64 = 1024 * 1024;

/// Default total number of origin GETs allowed to run concurrently. See
/// `RangeCache::new`.
pub const DEFAULT_TOTAL_FETCH_PERMITS: usize = 32;
/// Default number of `DEFAULT_TOTAL_FETCH_PERMITS` slots reserved for demand
/// reads (never consumed by prefetch).
pub const DEFAULT_DEMAND_RESERVED_FETCH_PERMITS: usize = 8;
/// Default max byte span of one coalesced run GET.
pub const DEFAULT_MAX_COALESCED_GET_BYTES: u64 = 8 * 1024 * 1024;
/// Default promotion granularity: promote only the run(s) covering a demanded
/// block, not the whole prefetch batch.
pub const DEFAULT_PROMOTE_WHOLE_BATCH: bool = false;

/// Concurrency for probing the cache backend for block hits before going to
/// origin. Backend probes do real disk I/O on a foyer RAM-tier miss, so a
/// large read probed sequentially would serialize hundreds of disk reads.
const BACKEND_PROBE_CONCURRENCY: usize = 16;

/// Relative urgency of an origin fetch. Lower is more urgent.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Priority {
    Demand = 0,
    Prefetch = 1,
}

impl Priority {
    fn from_u8(v: u8) -> Self {
        if v == Priority::Demand as u8 {
            Priority::Demand
        } else {
            Priority::Prefetch
        }
    }
}

/// The set of entries submitted in one prefetch call. Used only when
/// `promote_whole_batch` is enabled: a demand joiner into any sibling entry
/// promotes every other entry in the batch, not just the one it joined.
///
/// Siblings are tracked by `Weak<InFlight>`, captured once each entry is
/// created or joined, rather than by key string: a key can be removed from
/// `FetchScheduler::inflight` and later reused by an unrelated fetch (e.g.
/// after eviction), and promoting by key alone would then spuriously promote
/// that new, logically unrelated `InFlight`.
struct BatchState {
    entries: StdMutex<Vec<Weak<InFlight>>>,
}

type FetchResult = Result<Bytes, Arc<anyhow::Error>>;

/// One outstanding origin fetch (a single block or a `size()` head), shared
/// across every concurrent caller asking for the same key.
struct InFlight {
    priority: AtomicU8,
    promote: Notify,
    result: watch::Sender<Option<FetchResult>>,
    batch: Option<Arc<BatchState>>,
    /// Set by the first `fulfill()` call. Lets a `FulfillGuard`'s panic-path
    /// fallback tell which entries in a partially-completed run already got
    /// a real result, so it never clobbers one with a synthesized error.
    fulfilled: std::sync::atomic::AtomicBool,
}

impl InFlight {
    fn new(priority: Priority, batch: Option<Arc<BatchState>>) -> Self {
        let (result, _rx) = watch::channel(None);
        Self {
            priority: AtomicU8::new(priority as u8),
            promote: Notify::new(),
            result,
            batch,
            fulfilled: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn priority(&self) -> Priority {
        Priority::from_u8(self.priority.load(Ordering::Acquire))
    }

    fn promote_to_demand(&self) {
        self.priority
            .store(Priority::Demand as u8, Ordering::Release);
        self.promote.notify_one();
    }

    /// Deliver `result` to every joiner. Idempotent: only the first call
    /// actually sends — later calls (notably a `FulfillGuard`'s panic-path
    /// fallback racing a fetch that in fact completed normally) are no-ops,
    /// so a real result is never overwritten by a synthesized error.
    fn fulfill(&self, result: FetchResult) {
        if self.fulfilled.swap(true, Ordering::AcqRel) {
            return;
        }
        // A send error just means every waiter already dropped its
        // receiver, which is harmless.
        let _ = self.result.send(Some(result));
    }

    /// Wait for the fetch to complete, returning immediately if it already
    /// has. Safe against the "subscribe after send" race: a fresh `watch`
    /// receiver's initial value counts as unseen until `borrow_and_update`
    /// checks it, so a result sent before this call is still observed here.
    async fn join(&self) -> FetchResult {
        let mut rx = self.result.subscribe();
        loop {
            if let Some(r) = rx.borrow_and_update().clone() {
                return r;
            }
            if rx.changed().await.is_err() {
                return Err(Arc::new(anyhow!(
                    "in-flight fetch entry dropped without a result"
                )));
            }
        }
    }
}

enum Ownership {
    Owner(Arc<InFlight>),
    Joiner(Arc<InFlight>),
}

/// Owns the in-flight single-flight map and the priority-aware origin-fetch
/// budget. Replaces the two moka caches that used to live directly on
/// `RangeCache`.
struct FetchScheduler {
    inflight: StdMutex<HashMap<String, Arc<InFlight>>>,
    /// Bounds total concurrent origin GETs (blocks + size heads don't count
    /// against this; only run GETs acquire it).
    shared_permits: Arc<Semaphore>,
    /// Prefetch runs must additionally hold one of these; sized to
    /// `total - demand_reserved` so demand always finds a free shared permit.
    prefetch_permits: Arc<Semaphore>,
    promote_whole_batch: bool,
}

impl FetchScheduler {
    fn new(total: usize, demand_reserved: usize, promote_whole_batch: bool) -> Self {
        assert!(total > 0, "fetch concurrency total must be > 0");
        assert!(
            demand_reserved <= total,
            "demand_reserved ({demand_reserved}) must be <= total ({total})"
        );
        Self {
            inflight: StdMutex::new(HashMap::new()),
            shared_permits: Arc::new(Semaphore::new(total)),
            prefetch_permits: Arc::new(Semaphore::new(total - demand_reserved)),
            promote_whole_batch,
        }
    }

    /// Look up `key` in the in-flight map: become the owner if absent, or a
    /// joiner if present. A demand joiner into a prefetch-priority entry
    /// promotes it (and, if `promote_whole_batch`, its batch siblings) to
    /// demand so it competes for reserved capacity instead of sitting behind
    /// other prefetch work.
    fn own_or_join(
        &self,
        key: String,
        prio: Priority,
        batch: Option<Arc<BatchState>>,
    ) -> Ownership {
        let mut promote_batch: Option<(Arc<BatchState>, Arc<InFlight>)> = None;
        let ownership = {
            let mut map = self.inflight.lock().expect("inflight lock");
            if let Some(existing) = map.get(&key) {
                if prio == Priority::Demand && existing.priority() == Priority::Prefetch {
                    existing.promote_to_demand();
                    if self.promote_whole_batch
                        && let Some(bs) = existing.batch.clone()
                    {
                        promote_batch = Some((bs, existing.clone()));
                    }
                }
                Ownership::Joiner(existing.clone())
            } else {
                let entry = Arc::new(InFlight::new(prio, batch));
                map.insert(key.clone(), entry.clone());
                Ownership::Owner(entry)
            }
        };
        if let Some((bs, skip)) = promote_batch {
            self.promote_batch_siblings(&bs, &skip);
        }
        ownership
    }

    /// Promote every sibling in `batch` other than `skip` to demand priority.
    /// Siblings are resolved through `Weak<InFlight>` references captured at
    /// batch-membership time, not by re-looking up their key in `inflight`:
    /// the key may since have been removed and reused by an unrelated fetch,
    /// and identity (not the key string) is what must match here.
    fn promote_batch_siblings(&self, batch: &BatchState, skip: &Arc<InFlight>) {
        let entries = batch.entries.lock().expect("batch lock");
        for weak in entries.iter() {
            if let Some(entry) = weak.upgrade()
                && !Arc::ptr_eq(&entry, skip)
            {
                entry.promote_to_demand();
            }
        }
    }

    fn remove_entry(&self, key: &str) {
        self.inflight.lock().expect("inflight lock").remove(key);
    }
}

/// Scope guard held by the task that owns one or more in-flight entries
/// (either a single `size()` head or the members of one coalesced block
/// run). If the owning task exits normally, it calls `disarm()` after
/// fulfilling every entry and removing it from `FetchScheduler::inflight`.
///
/// If instead the task panics (e.g. `Bytes::slice` on a short origin read)
/// tokio catches the unwind at the task boundary and silently drops the
/// result, but this guard's `Drop` still runs during that unwind: it
/// fulfills any not-yet-fulfilled entry with an error (the `fulfilled` flag
/// on `InFlight` makes this a no-op for entries that already got a real
/// result) and removes every entry from the map. Without this, a panicking
/// owner would leave `fulfill()` never called, hanging every joiner
/// (including the owner itself) forever and leaking the entry permanently.
struct FulfillGuard {
    scheduler: Arc<FetchScheduler>,
    entries: Vec<(String, Arc<InFlight>)>,
    armed: bool,
}

impl FulfillGuard {
    fn new(scheduler: Arc<FetchScheduler>, entries: Vec<(String, Arc<InFlight>)>) -> Self {
        Self {
            scheduler,
            entries,
            armed: true,
        }
    }

    /// Call once the normal completion path has fulfilled and removed every
    /// entry, so the guard's `Drop` becomes a no-op.
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for FulfillGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        warn!(
            "fetch task exited without completing normally (likely a panic); \
             fulfilling {} in-flight entries with an error",
            self.entries.len()
        );
        for (key, entry) in &self.entries {
            entry.fulfill(Err(Arc::new(anyhow!(
                "fetch task exited without producing a result (panic during fetch)"
            ))));
            self.scheduler.remove_entry(key);
        }
    }
}

/// Held for the duration of one origin GET; dropping it frees the slot(s) for
/// the next waiter. Fields are never read, only held for their `Drop` effect.
struct RunPermit {
    _shared: OwnedSemaphorePermit,
    _prefetch: Option<OwnedSemaphorePermit>,
}

/// Resolves when any of `entries`' priority has been promoted since this call
/// started. Constructing the `Notified` future(s) up front (before the caller
/// re-checks priority) is what makes this race-free: tokio guarantees a
/// `notify_one()` that happens after a `Notified` is created is never missed,
/// even if that `Notified` is later dropped without completing.
async fn any_entry_promoted(entries: &[Arc<InFlight>]) {
    match entries {
        [] => std::future::pending::<()>().await,
        [only] => only.promote.notified().await,
        many => {
            let futs: Vec<_> = many
                .iter()
                .map(|e| Box::pin(e.promote.notified()))
                .collect();
            futures::future::select_all(futs).await;
        }
    }
}

/// Acquire the permit(s) needed to run one coalesced GET covering `entries`,
/// honoring promotion: the run's effective priority is the most urgent
/// (minimum) of its entries', re-checked every time a promotion wakes this
/// loop so a promotion mid-wait drops the prefetch-class requirement.
async fn acquire_run_permit(scheduler: &FetchScheduler, entries: &[Arc<InFlight>]) -> RunPermit {
    loop {
        let all_prefetch = entries.iter().all(|e| e.priority() == Priority::Prefetch);
        if !all_prefetch {
            let shared = scheduler
                .shared_permits
                .clone()
                .acquire_owned()
                .await
                .expect("shared_permits semaphore is never closed");
            return RunPermit {
                _shared: shared,
                _prefetch: None,
            };
        }

        tokio::select! {
            prefetch = scheduler.prefetch_permits.clone().acquire_owned() => {
                let prefetch = prefetch.expect("prefetch_permits semaphore is never closed");
                tokio::select! {
                    shared = scheduler.shared_permits.clone().acquire_owned() => {
                        let shared = shared.expect("shared_permits semaphore is never closed");
                        return RunPermit { _shared: shared, _prefetch: Some(prefetch) };
                    }
                    _ = any_entry_promoted(entries) => {
                        drop(prefetch);
                        continue;
                    }
                }
            }
            _ = any_entry_promoted(entries) => {
                continue;
            }
        }
    }
}

/// Reconstruct an owned error from a shared in-flight failure. `anyhow::Error`
/// is not `Clone`, so a joiner cannot move it out of the `Arc`; and it must
/// not be stringified, or a missing key would stop downcasting to
/// `object_store::Error::NotFound` and would surface as a 500 instead of a
/// 404 (see `validation::is_not_found`).
fn reconstruct_shared_error(shared: &Arc<anyhow::Error>) -> anyhow::Error {
    if let Some(object_store::Error::NotFound { path, source }) =
        shared.downcast_ref::<object_store::Error>()
    {
        let rebuilt = object_store::Error::NotFound {
            path: path.clone(),
            source: Box::new(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                source.to_string(),
            )),
        };
        return anyhow::Error::from(rebuilt);
    }
    anyhow!("{shared}")
}

fn decode_size(data: &Bytes) -> Result<u64> {
    Ok(u64::from_le_bytes(
        data[..8].try_into().expect("8-byte size slice"),
    ))
}

/// Range-aware read cache over an origin object store.
///
/// # Cache invalidation
///
/// This cache assumes object keys are **write-once and content-addressed**: a
/// given key always maps to the same bytes for the lifetime of the object. The
/// size and block caches therefore carry no TTL, etag, or generation in their
/// keys and are never invalidated. Overwriting an existing key with different
/// content would cause stale size/block data to be served indefinitely. This is
/// safe for micromegas lake objects (blocks, parquet) which are never
/// overwritten; do not point this cache at a mutable namespace.
///
/// # In-flight map and priority
///
/// Concurrent fetches of the same block or size are collapsed via an
/// in-flight map (`FetchScheduler`): the first caller to ask for a key
/// becomes its owner and issues the origin request (spawned as a detached
/// task, so a cancelled caller never strands the others waiting on it);
/// every other concurrent caller joins and observes the same result.
/// Contiguous missing blocks the owner controls are coalesced into one
/// `origin.get_range` per run. Every origin GET is either `Demand` or
/// `Prefetch` priority; a demand caller joining a prefetch-priority fetch
/// promotes it (see `own_or_join`), so a late demand read is never stuck
/// behind unrelated prefetch traffic.
#[derive(Clone)]
pub struct RangeCache {
    origin: Arc<dyn ObjectStore>,
    backend: Arc<dyn RangeCacheBackend>,
    block_size: u64,
    ns: String,
    scheduler: Arc<FetchScheduler>,
    max_coalesced_get_bytes: u64,
}

impl RangeCache {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        origin: Arc<dyn ObjectStore>,
        backend: Arc<dyn RangeCacheBackend>,
        block_size: u64,
        ns: String,
        total_fetch_permits: usize,
        demand_reserved_fetch_permits: usize,
        max_coalesced_get_bytes: u64,
        promote_whole_batch: bool,
    ) -> Self {
        Self {
            origin,
            backend,
            block_size,
            ns,
            scheduler: Arc::new(FetchScheduler::new(
                total_fetch_permits,
                demand_reserved_fetch_permits,
                promote_whole_batch,
            )),
            max_coalesced_get_bytes,
        }
    }

    /// Size in bytes of one cache block. Every distinct block a request
    /// touches is fetched and held whole, so callers gating memory (e.g. the
    /// server's cross-request budget) need this to account for amplification
    /// from small scattered ranges.
    pub fn block_size(&self) -> u64 {
        self.block_size
    }

    #[span_fn]
    pub async fn size(&self, key: &str) -> Result<u64> {
        // The cache key carries no etag/version: see the module docs — keys
        // are assumed write-once and content-addressed, so a cached size is
        // never invalidated.
        let meta_key = format!("meta:{}:{key}", self.ns);

        if let Some(data) = self.backend.get(&meta_key).await
            && data.len() == 8
        {
            imetric!("range_cache_size_backend_hit", "count", 1_u64);
            return decode_size(&data);
        }

        match self
            .scheduler
            .own_or_join(meta_key.clone(), Priority::Demand, None)
        {
            Ownership::Owner(entry) => {
                let origin = self.origin.clone();
                let backend = self.backend.clone();
                let scheduler = self.scheduler.clone();
                let key_owned = key.to_string();
                let meta_key_owned = meta_key.clone();
                let task_entry = entry.clone();
                tokio::spawn(async move {
                    let guard = FulfillGuard::new(
                        scheduler.clone(),
                        vec![(meta_key_owned.clone(), task_entry.clone())],
                    );
                    imetric!("range_cache_origin_head", "count", 1_u64);
                    match origin.head(&Path::from(key_owned.as_str())).await {
                        Ok(object_meta) => {
                            let size = object_meta.size;
                            debug!("range_cache origin head key={key_owned} size={size}");
                            let size_bytes = Bytes::from(size.to_le_bytes().to_vec());
                            backend
                                .put(meta_key_owned.clone(), size_bytes.clone(), FillHint::Demand)
                                .await;
                            task_entry.fulfill(Ok(size_bytes));
                        }
                        Err(e) => {
                            task_entry.fulfill(Err(Arc::new(anyhow::Error::from(e))));
                        }
                    }
                    scheduler.remove_entry(&meta_key_owned);
                    guard.disarm();
                });
                let data = entry
                    .join()
                    .await
                    .map_err(|e| reconstruct_shared_error(&e))?;
                decode_size(&data)
            }
            Ownership::Joiner(entry) => {
                let data = entry
                    .join()
                    .await
                    .map_err(|e| reconstruct_shared_error(&e))?;
                decode_size(&data)
            }
        }
    }

    /// Fetch `indices` (block indices within `key`, sized `file_size`) at the
    /// given priority, returning the bytes for every requested block on the
    /// `Demand` path. On the `Prefetch` path bytes are written to the backend
    /// and dropped as each owned run completes; the returned map is always
    /// empty, which is what keeps the prefetch peak bounded by
    /// `prefetch_concurrency * max_coalesced_get_bytes` rather than the full
    /// request size.
    async fn fetch_blocks(
        &self,
        key: &str,
        file_size: u64,
        indices: &[u64],
        prio: Priority,
    ) -> Result<HashMap<u64, Bytes>> {
        if indices.is_empty() {
            return Ok(HashMap::new());
        }

        let mut hits: HashMap<u64, Bytes> = HashMap::new();
        let mut missing: Vec<u64> = Vec::new();
        {
            // Probe the backend with bounded concurrency: a foyer RAM-tier
            // miss falls through to async disk I/O, and probing a large read
            // one block at a time would serialize those disk reads. The
            // futures are collected eagerly (not mapped lazily inside the
            // stream) so the resulting future stays `Send`-inferable across
            // `tokio::spawn` (rustc's HRTB limitation with borrowed closures).
            let probe_futs: Vec<_> = indices
                .iter()
                .map(|&idx| {
                    let block_key = format!("blk:{}:{key}:{idx}", self.ns);
                    let backend = self.backend.clone();
                    async move {
                        imetric!("range_cache_block_request", "count", 1_u64);
                        (idx, backend.get(&block_key).await)
                    }
                })
                .collect();
            let mut probes = stream::iter(probe_futs).buffer_unordered(BACKEND_PROBE_CONCURRENCY);
            while let Some((idx, probe)) = probes.next().await {
                match probe {
                    Some(data) => {
                        imetric!("range_cache_block_backend_hit", "count", 1_u64);
                        // A prefetch has nothing to do with an already-cached
                        // block, so drop the bytes immediately instead of
                        // accumulating every hit for the whole call.
                        if prio == Priority::Demand {
                            hits.insert(idx, data);
                        }
                    }
                    None => missing.push(idx),
                }
            }
        }

        if missing.is_empty() {
            return Ok(hits);
        }
        missing.sort_unstable();
        missing.dedup();

        // Only prefetch calls carry a batch handle: it's what lets a demand
        // joiner promote every sibling of a scattered prefetch call, not just
        // the one block it happened to touch (see `promote_whole_batch`).
        let batch = if prio == Priority::Prefetch {
            Some(Arc::new(BatchState {
                entries: StdMutex::new(Vec::with_capacity(missing.len())),
            }))
        } else {
            None
        };

        let mut owned: Vec<u64> = Vec::new();
        let mut entries: HashMap<u64, Arc<InFlight>> = HashMap::with_capacity(missing.len());
        for &idx in &missing {
            let block_key = format!("blk:{}:{key}:{idx}", self.ns);
            match self.scheduler.own_or_join(block_key, prio, batch.clone()) {
                Ownership::Owner(entry) => {
                    owned.push(idx);
                    entries.insert(idx, entry);
                }
                Ownership::Joiner(entry) => {
                    entries.insert(idx, entry);
                }
            }
        }
        // Record every member of this batch (owned or joined) as a promotion
        // sibling, keyed by identity rather than by key string.
        if let Some(bs) = &batch {
            let mut list = bs.entries.lock().expect("batch lock");
            list.extend(entries.values().map(Arc::downgrade));
        }

        let hint = match prio {
            Priority::Demand => FillHint::Demand,
            Priority::Prefetch => FillHint::Prefetch,
        };

        // `owned` is a sorted subsequence of `missing` (itself sorted), so no
        // extra sort is needed before coalescing.
        for run in coalesce_runs(&owned, self.block_size, self.max_coalesced_get_bytes) {
            let run_len = (run.end - run.start) as usize;
            let run_entries: Vec<Arc<InFlight>> = (run.start..run.end)
                .map(|idx| entries.get(&idx).expect("owned entry present").clone())
                .collect();
            let run_keys: Vec<String> = (run.start..run.end)
                .map(|idx| format!("blk:{}:{key}:{idx}", self.ns))
                .collect();
            let origin = self.origin.clone();
            let backend = self.backend.clone();
            let scheduler = self.scheduler.clone();
            let block_size = self.block_size;
            let key_owned = key.to_string();

            // Spawned as a detached task so a cancelled caller (e.g. an axum
            // request future dropped on client disconnect) can never strand
            // the other owned blocks or any joiners waiting on this run.
            tokio::spawn(async move {
                let guard = FulfillGuard::new(
                    scheduler.clone(),
                    run_keys
                        .iter()
                        .cloned()
                        .zip(run_entries.iter().cloned())
                        .collect(),
                );
                let permit = acquire_run_permit(&scheduler, &run_entries).await;
                let byte_start = run.start * block_size;
                let byte_end = block_byte_range(run.end - 1, block_size, file_size).end;
                let path = Path::from(key_owned.as_str());
                let outcome = origin.get_range(&path, byte_start..byte_end).await;
                drop(permit);

                match outcome {
                    Ok(data) => {
                        imetric!("range_cache_origin_block_fetch", "count", run_len as u64);
                        imetric!("range_cache_origin_block_bytes", "bytes", data.len() as u64);
                        debug!(
                            "range_cache origin fetch key={key_owned} run={run:?} bytes={}",
                            data.len()
                        );
                        for i in 0..run_len {
                            let offset = i as u64 * block_size;
                            let local_start = offset as usize;
                            let local_end = (offset + block_size).min(data.len() as u64) as usize;
                            let chunk = data.slice(local_start..local_end);
                            backend.put(run_keys[i].clone(), chunk.clone(), hint).await;
                            run_entries[i].fulfill(Ok(chunk));
                        }
                    }
                    Err(e) => {
                        let shared: Arc<anyhow::Error> = Arc::new(anyhow::Error::from(e));
                        for entry in &run_entries {
                            entry.fulfill(Err(shared.clone()));
                        }
                    }
                }
                for k in &run_keys {
                    scheduler.remove_entry(k);
                }
                guard.disarm();
            });
        }

        if prio == Priority::Prefetch {
            // Join entries as each completes (not in index order), dropping
            // the joined bytes and the `Arc<InFlight>` right away. The watch
            // channel inside an entry retains the fulfilled `Bytes` — a slice
            // sharing its whole run's GET buffer — for as long as the entry
            // is alive, so holding every entry until all runs finished (as
            // the demand path's `join_all` does) would let the prefetch peak
            // reach the full request size instead of the documented
            // `prefetch_concurrency * max_coalesced_get_bytes` bound. Any
            // demand joiner that registered before completion still gets the
            // bytes: it holds its own `Arc<InFlight>` and reads the channel
            // independently of this early drop.
            let mut joins: FuturesUnordered<_> = entries
                .into_values()
                .map(|entry| async move { entry.join().await.map(|_bytes| ()) })
                .collect();
            while let Some(r) = joins.next().await {
                r.map_err(|e| reconstruct_shared_error(&e))?;
            }
            return Ok(HashMap::new());
        }

        let joined = join_all(missing.iter().map(|idx| {
            let entry = entries
                .get(idx)
                .expect("entry present for every missing index")
                .clone();
            let idx = *idx;
            async move { (idx, entry.join().await) }
        }))
        .await;

        for (idx, r) in joined {
            let data = r.map_err(|e| reconstruct_shared_error(&e))?;
            hits.insert(idx, data);
        }
        Ok(hits)
    }

    #[span_fn]
    pub async fn get_range(&self, key: &str, range: Range<u64>) -> Result<Bytes> {
        let file_size = match self.size(key).await {
            Ok(s) => s,
            Err(e) => {
                imetric!("range_cache_get_range_error", "count", 1_u64);
                return Err(e);
            }
        };

        let start = range.start;
        let end = range.end;

        if end > file_size {
            imetric!("range_cache_get_range_error", "count", 1_u64);
            return Err(RangeError::OutOfBounds {
                requested_end: end,
                file_size,
            }
            .into());
        }

        if start >= end {
            return Ok(Bytes::new());
        }

        let blk_indices = blocks_for_range(start, end, self.block_size);
        let indices: Vec<u64> = (blk_indices.start..blk_indices.end).collect();

        let block_map = match self
            .fetch_blocks(key, file_size, &indices, Priority::Demand)
            .await
        {
            Ok(m) => m,
            Err(e) => {
                imetric!("range_cache_get_range_error", "count", 1_u64);
                return Err(e);
            }
        };

        let blocks: Vec<(u64, Bytes)> = indices
            .into_iter()
            .map(|idx| {
                let data = block_map
                    .get(&idx)
                    .cloned()
                    .expect("fetch_blocks returns every requested index");
                (idx, data)
            })
            .collect();
        Ok(assemble_range(&blocks, self.block_size, start, end))
    }

    #[span_fn]
    pub async fn get_ranges(&self, key: &str, ranges: &[Range<u64>]) -> Result<Vec<Bytes>> {
        if ranges.is_empty() {
            return Ok(vec![]);
        }

        // Propagate the size-lookup error unwrapped so the underlying
        // `object_store::Error` (notably `NotFound`) survives the downcast in
        // callers, matching `get_range` and the single-GET endpoint.
        let file_size = match self.size(key).await {
            Ok(s) => s,
            Err(e) => {
                imetric!("range_cache_get_ranges_error", "count", 1_u64);
                return Err(e);
            }
        };

        let mut all_block_indices = BTreeSet::new();
        for r in ranges {
            let start = r.start;
            let end = r.end;
            if end > file_size {
                imetric!("range_cache_get_ranges_error", "count", 1_u64);
                return Err(RangeError::OutOfBounds {
                    requested_end: end,
                    file_size,
                }
                .into());
            }
            if start < end {
                let blk = blocks_for_range(start, end, self.block_size);
                for idx in blk.start..blk.end {
                    all_block_indices.insert(idx);
                }
            }
        }

        let indices: Vec<u64> = all_block_indices.into_iter().collect();
        let block_map = match self
            .fetch_blocks(key, file_size, &indices, Priority::Demand)
            .await
        {
            Ok(m) => m,
            Err(e) => {
                imetric!("range_cache_get_ranges_error", "count", 1_u64);
                return Err(e);
            }
        };

        let mut result = Vec::with_capacity(ranges.len());
        for r in ranges {
            let start = r.start;
            let end = r.end;
            if start >= end {
                result.push(Bytes::new());
                continue;
            }
            let blk = blocks_for_range(start, end, self.block_size);
            let blocks: Vec<(u64, Bytes)> = (blk.start..blk.end)
                .filter_map(|idx| block_map.get(&idx).map(|d| (idx, d.clone())))
                .collect();
            result.push(assemble_range(&blocks, self.block_size, start, end));
        }

        Ok(result)
    }

    /// Warm the cache for `ranges` at `Prefetch` priority without returning
    /// any bytes. The HTTP surface for this (endpoint + client method) is
    /// #1198; this is the priority-carrying core it builds on. Public (rather
    /// than crate-private) so integration tests under `tests/` — which
    /// compile as a separate crate — can exercise the promotion behavior
    /// described in the fetch-rework plan.
    pub async fn prefetch_ranges(&self, key: &str, ranges: &[Range<u64>]) -> Result<()> {
        if ranges.is_empty() {
            return Ok(());
        }
        let file_size = self.size(key).await?;
        let mut all_block_indices = BTreeSet::new();
        for r in ranges {
            let start = r.start;
            let end = r.end;
            if end > file_size {
                return Err(RangeError::OutOfBounds {
                    requested_end: end,
                    file_size,
                }
                .into());
            }
            if start < end {
                let blk = blocks_for_range(start, end, self.block_size);
                all_block_indices.extend(blk.start..blk.end);
            }
        }
        self.prefetch_blocks(
            key,
            file_size,
            &all_block_indices.into_iter().collect::<Vec<_>>(),
        )
        .await
    }

    /// Warm the cache for the given block indices at `Prefetch` priority.
    pub async fn prefetch_blocks(&self, key: &str, file_size: u64, indices: &[u64]) -> Result<()> {
        self.fetch_blocks(key, file_size, indices, Priority::Prefetch)
            .await?;
        Ok(())
    }
}
