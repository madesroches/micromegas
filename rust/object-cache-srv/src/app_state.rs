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
    /// Cross-request bound on concurrently-assembled response bytes: one
    /// permit per MiB. A handler acquires `ceil(bytes / 1 MiB)` permits before
    /// assembling a response and holds them for the response body's full
    /// lifetime.
    pub mem_permits: Arc<Semaphore>,
    /// Total capacity of `mem_permits`, in MiB. A request whose assembled size
    /// would exceed this outright is rejected (413) rather than acquired
    /// (which would otherwise block forever).
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
