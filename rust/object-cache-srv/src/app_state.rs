use std::sync::Arc;

use micromegas_object_cache::prefetch::PrefetchItem;
use micromegas_object_cache::range_cache::RangeCache;
use tokio::sync::{Semaphore, mpsc};

#[derive(Clone)]
pub struct AppState {
    pub cache: RangeCache,
    /// Prefixes a request key may fall under. Empty = allow every key; this is
    /// only reachable via `--allow-all-prefixes`, since the server refuses to
    /// start with an empty list otherwise.
    pub allowed_prefixes: Vec<String>,
    /// Cross-request bound on concurrent in-flight streaming windows: one
    /// permit per MiB. A handler acquires `ceil(min(response size, the fixed
    /// streaming window) / 1 MiB)` permits before starting to stream a
    /// response and holds them for the response body's full lifetime — a
    /// small response charges close to its actual size, a large one clamps
    /// to the window, so the charge reflects in-flight window bytes rather
    /// than the whole response size.
    pub mem_permits: Arc<Semaphore>,
    /// Total capacity of `mem_permits`, in MiB. The startup guard in
    /// `object_cache_srv.rs` floors this at the fixed streaming window's
    /// size, so a single large streaming request's charge can never exceed
    /// the whole budget (which would otherwise block `acquire_many_owned`
    /// forever).
    pub memory_budget_mb: u32,
    /// Sender side of the bounded prefetch queue; the consumer task (spawned
    /// separately, see `prefetch_queue::spawn_prefetch_worker`) owns the
    /// receiver. Every `AppState` clone (one per request) holds a clone of
    /// this sender, so the channel closes only once the server shuts down.
    pub prefetch_tx: mpsc::Sender<PrefetchItem>,
}

impl AppState {
    pub fn new(
        cache: RangeCache,
        allowed_prefixes: Vec<String>,
        memory_budget_mb: u32,
        prefetch_tx: mpsc::Sender<PrefetchItem>,
    ) -> Self {
        Self {
            cache,
            allowed_prefixes,
            mem_permits: Arc::new(Semaphore::new(memory_budget_mb as usize)),
            memory_budget_mb,
            prefetch_tx,
        }
    }
}
