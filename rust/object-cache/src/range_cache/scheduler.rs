use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
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
        // every later joiner forever. `send_replace` stores the value
        // unconditionally.
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

/// One priority-tiered pool of semaphore permits: `shared` sized `total`,
/// `prefetch` sized `total - demand_reserved` so prefetch work never starves
/// demand of its reserved slice. `FetchScheduler` instantiates this twice --
/// once denominated in runs (`count`), once in bytes (`bytes`) -- so the two
/// resources (origin-GET parallelism vs. transient fetch memory) are bounded
/// independently but share the same reservation shape.
struct PriorityBudget {
    shared: Arc<Semaphore>,
    /// Total capacity of `shared`, stored alongside it since
    /// `tokio::sync::Semaphore` has no capacity accessor. Used by `stats`
    /// for the saturation sampler.
    shared_total: usize,
    prefetch: Arc<Semaphore>,
    /// Total capacity of `prefetch`, for the same reason as `shared_total`.
    prefetch_total: usize,
}

impl PriorityBudget {
    fn new(total: usize, demand_reserved: usize) -> Self {
        assert!(total > 0, "fetch budget total must be > 0");
        // Strictly less: `demand_reserved == total` would leave the prefetch
        // semaphore with zero permits, hanging every prefetch run forever.
        assert!(
            demand_reserved < total,
            "demand_reserved ({demand_reserved}) must be < total ({total})"
        );
        let prefetch_total = total - demand_reserved;
        Self {
            shared: Arc::new(Semaphore::new(total)),
            shared_total: total,
            prefetch: Arc::new(Semaphore::new(prefetch_total)),
            prefetch_total,
        }
    }

    /// Acquire `n` permits for one coalesced GET covering `entries`, honoring
    /// promotion: the run's effective priority is the most urgent (minimum)
    /// of its entries', re-checked every time a promotion wakes this loop so
    /// a promotion mid-wait drops the prefetch-class requirement.
    async fn acquire(&self, n: u32, entries: &[Arc<InFlight>]) -> BudgetPermit {
        loop {
            let all_prefetch = entries.iter().all(|e| e.priority() == Priority::Prefetch);
            if !all_prefetch {
                let shared = self
                    .shared
                    .clone()
                    .acquire_many_owned(n)
                    .await
                    .expect("shared semaphore is never closed");
                return BudgetPermit {
                    _shared: shared,
                    _prefetch: None,
                };
            }

            tokio::select! {
                prefetch = self.prefetch.clone().acquire_many_owned(n) => {
                    let prefetch = prefetch.expect("prefetch semaphore is never closed");
                    tokio::select! {
                        shared = self.shared.clone().acquire_many_owned(n) => {
                            let shared = shared.expect("shared semaphore is never closed");
                            return BudgetPermit { _shared: shared, _prefetch: Some(prefetch) };
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

    fn stats(&self) -> BudgetStats {
        BudgetStats {
            shared_available: self.shared.available_permits(),
            shared_total: self.shared_total,
            prefetch_available: self.prefetch.available_permits(),
            prefetch_total: self.prefetch_total,
        }
    }
}

/// Held for the duration of one origin GET's share of one `PriorityBudget`;
/// dropping it frees the slot(s) for the next waiter. Fields are never read,
/// only held for their `Drop` effect.
pub(super) struct BudgetPermit {
    _shared: OwnedSemaphorePermit,
    _prefetch: Option<OwnedSemaphorePermit>,
}

/// A fetch-permit budget's current occupancy (one `PriorityBudget`'s worth),
/// for the saturation sampler (`object-cache-srv/src/saturation_monitor.rs`).
pub struct BudgetStats {
    pub shared_available: usize,
    pub shared_total: usize,
    pub prefetch_available: usize,
    pub prefetch_total: usize,
}

/// Both of `FetchScheduler`'s budgets' occupancy: `count` (one permit per
/// in-flight run, bounding origin-GET parallelism) and `bytes` (one permit
/// per in-flight byte, bounding transient fetch memory).
pub struct FetchBudgetStats {
    pub count: BudgetStats,
    pub bytes: BudgetStats,
}

/// Owns the in-flight single-flight map and the priority-aware origin-fetch
/// budget.
pub(super) struct FetchScheduler {
    inflight: StdMutex<HashMap<String, Arc<InFlight>>>,
    /// One permit per in-flight coalesced run (blocks + size heads don't
    /// count against this; only run GETs acquire it). Bounds origin-GET
    /// parallelism, independent of `bytes`.
    count: PriorityBudget,
    /// One permit per byte of an in-flight coalesced run's buffer. Bounds the
    /// transient origin-fetch memory ceiling, independent of `count`.
    bytes: PriorityBudget,
    promote_whole_batch: bool,
    /// Count of detached fetch tasks (`spawn_run_fetch` runs + `size()` HEADs)
    /// currently in flight, tracked by `FetchTaskGuard`. Used by graceful
    /// shutdown to wait for in-flight origin GETs instead of letting the
    /// tokio runtime drop them.
    outstanding_tasks: AtomicUsize,
    /// Notified when `outstanding_tasks` drops to zero. See `wait_drained`.
    drained: Notify,
}

impl FetchScheduler {
    /// `budget_bytes`/`reserved_bytes` are the byte-denominated analog of
    /// `total`/`demand_reserved`; `max_run_bytes` is the largest byte span one
    /// coalesced run can reach (`blocks::max_run_bytes`), asserted against
    /// both so a run can never exceed what either budget can ever grant.
    pub(super) fn new(
        total: usize,
        demand_reserved: usize,
        budget_bytes: u64,
        reserved_bytes: u64,
        max_run_bytes: u64,
        promote_whole_batch: bool,
    ) -> Self {
        // `acquire_many_owned` takes a `u32` permit count, so one run's byte
        // charge must fit in a `u32` -- checked here once at construction
        // rather than per-run in `acquire_run_permit`.
        assert!(
            max_run_bytes <= u32::MAX as u64,
            "max_run_bytes ({max_run_bytes}) must fit in u32: acquire_many_owned takes a u32 permit count"
        );
        assert!(
            reserved_bytes < budget_bytes,
            "fetch_memory_budget reserved_bytes ({reserved_bytes}) must be < budget_bytes ({budget_bytes})"
        );
        // Without this, a run larger than the prefetch byte pool would never
        // complete (and never error) acquiring `bytes` permits: the same hang
        // the `fetch_memory_budget_mb` floor in `object-cache-srv`'s
        // `cli.rs::validate` guards against at the CLI boundary.
        assert!(
            budget_bytes - reserved_bytes >= max_run_bytes,
            "fetch memory budget's prefetch pool ({} bytes = budget_bytes {budget_bytes} - \
             reserved_bytes {reserved_bytes}) must be >= max_run_bytes ({max_run_bytes}), or a \
             full-size run would hang forever acquiring its byte-budget permits",
            budget_bytes - reserved_bytes
        );
        Self {
            inflight: StdMutex::new(HashMap::new()),
            count: PriorityBudget::new(total, demand_reserved),
            bytes: PriorityBudget::new(
                usize::try_from(budget_bytes).expect("fetch_memory_budget_bytes fits in usize"),
                usize::try_from(reserved_bytes).expect("reserved fetch memory bytes fit in usize"),
            ),
            promote_whole_batch,
            outstanding_tasks: AtomicUsize::new(0),
            drained: Notify::new(),
        }
    }

    /// Register one detached fetch task as outstanding. Call *before*
    /// `tokio::spawn`, synchronously, so there is no window where a
    /// queued-but-not-yet-polled task is invisible to `wait_drained()`.
    pub(super) fn track_task(scheduler: &Arc<FetchScheduler>) -> FetchTaskGuard {
        scheduler.outstanding_tasks.fetch_add(1, Ordering::AcqRel);
        FetchTaskGuard(scheduler.clone())
    }

    /// Number of detached fetch tasks currently in flight to origin.
    pub(super) fn outstanding_tasks(&self) -> usize {
        self.outstanding_tasks.load(Ordering::Acquire)
    }

    /// Resolves once no detached fetch task is outstanding. Race-free by
    /// construction: `Notified::enable`'s docs guarantee that a
    /// `notify_waiters()` call is observed by a `Notified` future as long as
    /// that call happens after the `Notified` was created, regardless of
    /// whether `enable`/`poll` has run yet -- so creating the `Notified`
    /// *before* re-checking the count (rather than after) means a
    /// `notify_waiters()` racing the check in between is never missed. This
    /// is a different guarantee than `any_entry_promoted`'s below, which
    /// relies on `notify_one`'s stored-permit semantics; `notify_waiters`
    /// stores no permit, so the ordering above -- not a permit -- is what
    /// makes this race-free.
    pub(super) async fn wait_drained(&self) {
        loop {
            if self.outstanding_tasks() == 0 {
                return;
            }
            let notified = self.drained.notified();
            if self.outstanding_tasks() == 0 {
                return;
            }
            notified.await;
        }
    }

    /// Both fetch-permit budgets' current occupancy, for the saturation
    /// sampler (`object-cache-srv/src/saturation_monitor.rs`).
    pub(super) fn fetch_budget_stats(&self) -> FetchBudgetStats {
        FetchBudgetStats {
            count: self.count.stats(),
            bytes: self.bytes.stats(),
        }
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
        let n = self.entries.len();
        // `std::thread::panicking()` is `true` only while the current thread
        // is unwinding from a real panic; it is `false` when a task future is
        // simply dropped without being polled to completion (e.g. the tokio
        // runtime shutting down), so it's exactly the signal needed to tell
        // the two cases apart.
        let panicking = std::thread::panicking();
        if panicking {
            warn!("fetch task panicked; fulfilling {n} in-flight entries with an error");
        } else {
            warn!(
                "cache service shutting down; abandoning {n} in-flight fetch entries \
                 (joiners see a synthesized error and must refetch after restart)"
            );
        }
        let msg = if panicking {
            "fetch task panicked before producing a result"
        } else {
            "fetch task was abandoned (cache service shutting down)"
        };
        for (key, entry) in &self.entries {
            entry.fulfill(Err(Arc::new(anyhow!(msg))));
            self.scheduler.remove_entry(key);
        }
    }
}

/// Scope guard tracking the lifetime of one detached fetch task, held for
/// its whole body. Distinct from `FulfillGuard`, which tracks *entry
/// fulfillment*, not task lifetime -- the two guards serve different
/// purposes and both must survive a panicking OR shutdown-dropped task.
/// Because this is a plain local variable inside the spawned async block,
/// its `Drop` runs whether the task finishes normally, panics, or is dropped
/// without being polled to completion, so `outstanding_tasks` stays accurate
/// in all three cases without any special-casing.
pub(super) struct FetchTaskGuard(Arc<FetchScheduler>);

impl Drop for FetchTaskGuard {
    fn drop(&mut self) {
        if self.0.outstanding_tasks.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.drained.notify_waiters();
        }
    }
}

/// Held for the duration of one coalesced run: `count` bounds origin-GET
/// parallelism and is dropped right after the GET returns; `bytes` bounds the
/// run's transient memory and is held through backend admission. See
/// `acquire_run_permit` and its caller in `fetch.rs`.
pub(super) struct RunPermit {
    pub(super) count: BudgetPermit,
    pub(super) bytes: BudgetPermit,
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

/// Acquire the permits needed to run one coalesced GET covering `entries`,
/// spanning `run_bytes` bytes: bytes first, then count. Every run acquires in
/// this order, so there is no hold-and-wait cycle between the two budgets;
/// the cost is that a run holding its bytes while queued for a count slot
/// parks that byte budget idle for the wait, which only bites when the byte
/// budget is deliberately sized larger than `count * typical run`. Each
/// underlying `PriorityBudget::acquire` call independently honors promotion
/// (see its doc).
pub(super) async fn acquire_run_permit(
    scheduler: &FetchScheduler,
    entries: &[Arc<InFlight>],
    run_bytes: u64,
) -> RunPermit {
    let n = u32::try_from(run_bytes)
        .expect("run size validated <= u32::MAX at construction (FetchScheduler::new asserts max_run_bytes fits)");
    let bytes = scheduler.bytes.acquire(n, entries).await;
    let count = scheduler.count.acquire(1, entries).await;
    RunPermit { count, bytes }
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
