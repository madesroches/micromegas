//! Per-query [`MemoryPool`] wrapper that adds accounting on top of a shared pool.
//!
//! `ScopedMemoryPool` delegates every operation to the wrapped `inner` pool, so the
//! budget, spilling behavior, and top-consumer reporting (when `inner` is a
//! `TrackConsumersPool`) stay exactly as they were. The only thing it adds is its own
//! `current`/`peak` counters, which track just the reservations that were grown through
//! this particular instance. Because a `MemoryReservation` is permanently bound to the
//! `Arc<dyn MemoryPool>` it registered with (see `MemoryConsumer::register`), wrapping the
//! shared pool once per query and handing that wrapper to the query's `RuntimeEnv` is
//! enough to isolate one query's peak from another's, with no ambient context or naming
//! scheme involved.
use datafusion::error::Result;
use datafusion::execution::memory_pool::{
    MemoryConsumer, MemoryLimit, MemoryPool, MemoryReservation,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A [`MemoryPool`] that forwards every call to `inner`, while separately tracking the
/// current and peak (high-water mark) reservation grown through this instance alone.
#[derive(Debug)]
pub struct ScopedMemoryPool {
    inner: Arc<dyn MemoryPool>,
    current: AtomicUsize,
    peak: AtomicUsize,
}

impl ScopedMemoryPool {
    /// Wraps `inner`, starting both counters at zero.
    pub fn new(inner: Arc<dyn MemoryPool>) -> Self {
        Self {
            inner,
            current: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        }
    }

    /// Monotonic high-water mark of tracked reservation for this query, in bytes.
    pub fn peak(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }

    /// Currently outstanding tracked reservation, in bytes; `0` at quiescence.
    pub fn current(&self) -> usize {
        self.current.load(Ordering::Relaxed)
    }

    /// Records a successful grow of `additional` bytes: bumps `current` and, if that
    /// pushed it to a new high, `peak` along with it.
    fn record_grow(&self, additional: usize) {
        let cur = self.current.fetch_add(additional, Ordering::Relaxed) + additional;
        self.peak.fetch_max(cur, Ordering::Relaxed);
    }
}

impl MemoryPool for ScopedMemoryPool {
    fn name(&self) -> &str {
        "scoped"
    }

    // MUST forward: the trait's default `register`/`unregister` are no-ops, and
    // `TrackConsumersPool`'s whole tracked-consumers map (and therefore the top-consumers
    // text in its OOM error) is driven by them.
    fn register(&self, consumer: &MemoryConsumer) {
        self.inner.register(consumer);
    }

    fn unregister(&self, consumer: &MemoryConsumer) {
        self.inner.unregister(consumer);
    }

    fn grow(&self, reservation: &MemoryReservation, additional: usize) {
        self.inner.grow(reservation, additional);
        self.record_grow(additional);
    }

    fn try_grow(&self, reservation: &MemoryReservation, additional: usize) -> Result<()> {
        self.inner.try_grow(reservation, additional)?; // `?` keeps counters untouched on failure
        self.record_grow(additional);
        Ok(())
    }

    fn shrink(&self, reservation: &MemoryReservation, shrink: usize) {
        self.inner.shrink(reservation, shrink);
        let prev = self.current.fetch_sub(shrink, Ordering::Relaxed);
        debug_assert!(prev >= shrink, "scoped pool shrink underflow");
    }

    fn reserved(&self) -> usize {
        self.inner.reserved() // global truth, unchanged semantics
    }

    fn memory_limit(&self) -> MemoryLimit {
        self.inner.memory_limit()
    }
}

impl std::fmt::Display for ScopedMemoryPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "scoped(inner_pool: {}, peak: {})",
            self.inner,
            self.peak()
        )
    }
}
