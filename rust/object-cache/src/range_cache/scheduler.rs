use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};

use anyhow::{Result, anyhow};
use bytes::Bytes;
use micromegas_tracing::prelude::*;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, watch};

use crate::metric_tags;

/// Relative urgency of an origin fetch. Lower is more urgent.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum Priority {
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

    /// The `class` metric-tag value for this priority (see `metric_tags`).
    pub(super) fn class_label(self) -> &'static str {
        match self {
            Priority::Demand => metric_tags::CLASS_DEMAND,
            Priority::Prefetch => metric_tags::CLASS_PREFETCH,
        }
    }
}

/// The `class` tag for one coalesced run: `Demand` if any of its entries is
/// currently demand priority, else `Prefetch`. Mirrors `acquire_run_permit`'s
/// own `all_prefetch` check, evaluated once the permit has been acquired so a
/// promotion that raced the wait is reflected in the tag.
pub(super) fn effective_priority(entries: &[Arc<InFlight>]) -> Priority {
    if entries.iter().any(|e| e.priority() == Priority::Demand) {
        Priority::Demand
    } else {
        Priority::Prefetch
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
pub(super) struct BatchState {
    entries: StdMutex<Vec<Weak<InFlight>>>,
}

impl BatchState {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            entries: StdMutex::new(Vec::with_capacity(capacity)),
        }
    }
}

type FetchResult = Result<Bytes, Arc<anyhow::Error>>;

/// One outstanding origin fetch (a single block or a `size()` head), shared
/// across every concurrent caller asking for the same key.
pub(super) struct InFlight {
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
    pub(super) fn fulfill(&self, result: FetchResult) {
        if self.fulfilled.swap(true, Ordering::AcqRel) {
            return;
        }
        // `send_replace`, not `send`: `send` drops the value without storing
        // it when the channel currently has zero receivers, and joiners
        // subscribe lazily inside `join()`. A fetch task that completes
        // before any joiner's `subscribe()` would lose the result and hang
        // every later joiner forever (issue #1259). `send_replace` stores
        // the value unconditionally.
        self.result.send_replace(Some(result));
    }

    /// Wait for the fetch to complete, returning immediately if it already
    /// has. Safe against the "subscribe after send" race: a fresh `watch`
    /// receiver's initial value counts as unseen until `borrow_and_update`
    /// checks it, so a result sent before this call is still observed here.
    pub(super) async fn join(&self) -> FetchResult {
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

pub(super) enum Ownership {
    Owner(Arc<InFlight>),
    Joiner(Arc<InFlight>),
}

/// Owns the in-flight single-flight map and the priority-aware origin-fetch
/// budget. Replaces the two moka caches that used to live directly on
/// `RangeCache`.
pub(super) struct FetchScheduler {
    inflight: StdMutex<HashMap<String, Arc<InFlight>>>,
    /// Bounds total concurrent origin GETs (blocks + size heads don't count
    /// against this; only run GETs acquire it).
    shared_permits: Arc<Semaphore>,
    /// Total capacity of `shared_permits`, stored alongside it since
    /// `tokio::sync::Semaphore` has no capacity accessor. Used by
    /// `fetch_budget_stats` for the saturation sampler.
    shared_total: usize,
    /// Prefetch runs must additionally hold one of these; sized to
    /// `total - demand_reserved` so demand always finds a free shared permit.
    prefetch_permits: Arc<Semaphore>,
    /// Total capacity of `prefetch_permits`, for the same reason as
    /// `shared_total`.
    prefetch_total: usize,
    promote_whole_batch: bool,
}

impl FetchScheduler {
    pub(super) fn new(total: usize, demand_reserved: usize, promote_whole_batch: bool) -> Self {
        assert!(total > 0, "fetch concurrency total must be > 0");
        // Strictly less: `demand_reserved == total` would leave the prefetch
        // semaphore with zero permits, hanging every prefetch run forever.
        assert!(
            demand_reserved < total,
            "demand_reserved ({demand_reserved}) must be < total ({total})"
        );
        let prefetch_total = total - demand_reserved;
        Self {
            inflight: StdMutex::new(HashMap::new()),
            shared_permits: Arc::new(Semaphore::new(total)),
            shared_total: total,
            prefetch_permits: Arc::new(Semaphore::new(prefetch_total)),
            prefetch_total,
            promote_whole_batch,
        }
    }

    /// `(shared_available, shared_total, prefetch_available, prefetch_total)`
    /// -- the fetch-permit budget's current occupancy, for the saturation
    /// sampler (`object-cache-srv/src/saturation_monitor.rs`).
    pub(super) fn fetch_budget_stats(&self) -> (usize, usize, usize, usize) {
        (
            self.shared_permits.available_permits(),
            self.shared_total,
            self.prefetch_permits.available_permits(),
            self.prefetch_total,
        )
    }

    /// Number of keys (blocks or `size()` heads) currently in flight to
    /// origin.
    pub(super) fn inflight_len(&self) -> usize {
        self.inflight.lock().expect("inflight lock").len()
    }

    /// Look up `key` in the in-flight map: become the owner if absent, or a
    /// joiner if present. A demand joiner into a prefetch-priority entry
    /// promotes it (and, if `promote_whole_batch`, its batch siblings) to
    /// demand so it competes for reserved capacity instead of sitting behind
    /// other prefetch work.
    ///
    /// The entry is registered in `batch` (owner or joiner alike) while the
    /// inflight lock is still held, i.e. before any concurrent demand joiner
    /// can find the entry and run `promote_batch_siblings` over a
    /// partially-populated list.
    pub(super) fn own_or_join(
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
                if let Some(bs) = &batch {
                    bs.entries
                        .lock()
                        .expect("batch lock")
                        .push(Arc::downgrade(existing));
                }
                Ownership::Joiner(existing.clone())
            } else {
                let entry = Arc::new(InFlight::new(prio, batch.clone()));
                if let Some(bs) = &batch {
                    bs.entries
                        .lock()
                        .expect("batch lock")
                        .push(Arc::downgrade(&entry));
                }
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

    pub(super) fn remove_entry(&self, key: &str) {
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
pub(super) struct FulfillGuard {
    scheduler: Arc<FetchScheduler>,
    entries: Vec<(String, Arc<InFlight>)>,
    armed: bool,
}

impl FulfillGuard {
    pub(super) fn new(
        scheduler: Arc<FetchScheduler>,
        entries: Vec<(String, Arc<InFlight>)>,
    ) -> Self {
        Self {
            scheduler,
            entries,
            armed: true,
        }
    }

    /// Call once the normal completion path has fulfilled and removed every
    /// entry, so the guard's `Drop` becomes a no-op.
    pub(super) fn disarm(mut self) {
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
pub(super) struct RunPermit {
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
pub(super) async fn acquire_run_permit(
    scheduler: &FetchScheduler,
    entries: &[Arc<InFlight>],
) -> RunPermit {
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
pub(super) fn reconstruct_shared_error(shared: &Arc<anyhow::Error>) -> anyhow::Error {
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
    // `{shared:?}` keeps the full context chain (and backtrace, if captured)
    // so joiners' 500 log lines are as informative as the owner's.
    anyhow!("{shared:?}")
}

pub(super) fn decode_size(data: &Bytes) -> Result<u64> {
    Ok(u64::from_le_bytes(
        data[..8].try_into().expect("8-byte size slice"),
    ))
}
