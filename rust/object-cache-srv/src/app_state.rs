use std::sync::Arc;

use micromegas_object_cache::range_cache::RangeCache;
use tokio::sync::Semaphore;

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
}

impl AppState {
    pub fn new(cache: RangeCache, allowed_prefixes: Vec<String>, memory_budget_mb: u32) -> Self {
        Self {
            cache,
            allowed_prefixes,
            mem_permits: Arc::new(Semaphore::new(memory_budget_mb as usize)),
            memory_budget_mb,
        }
    }
}
