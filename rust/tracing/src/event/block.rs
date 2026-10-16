use crate::dyn_deps::DynDeps;
use crate::prelude::*;

/// Blocks are a batch of events sent as a unit
#[derive(Debug)]
pub struct EventBlock<Q: ExtractDeps> {
    pub process_id: uuid::Uuid,
    pub stream_id: uuid::Uuid,
    pub begin: DualTime,
    pub events: Q,
    pub end: Option<DualTime>,
    pub event_offset: usize,
    /// Dependency records for the dynamic property sets and dynamic strings that `events` refer
    /// to by id, inserted by log, metrics and thread dispatch.
    pub dyn_deps: DynDeps<Q::DepsQueue>,
}

impl<Q> EventBlock<Q>
where
    Q: micromegas_transit::HeterogeneousQueue + ExtractDeps,
{
    pub fn close(&mut self) {
        self.end = Some(DualTime::now());
    }

    /// Close this block with an already-stamped `DualTime`, so that it can be
    /// the same value used as the replacement block's `begin` — making
    /// `block[k].end == block[k+1].begin` true by construction. Used by the
    /// four `dispatch.rs` flush paths; `close()` is retained for standalone
    /// callers (benches/tests) that swap no stream and so have no shared
    /// timestamp to pass.
    pub fn close_at(&mut self, end: DualTime) {
        self.end = Some(end);
    }
}

/// Serialize the dependencies of the events in the primary queue
pub trait ExtractDeps {
    type DepsQueue: micromegas_transit::HeterogeneousQueue;
    /// `dyn_deps` holds the records of the dynamic ids that the events refer to. They are
    /// appended after the static dependencies, which they may reference.
    fn extract(&self, dyn_deps: &DynDeps<Self::DepsQueue>) -> Self::DepsQueue;
}

/// Event blocks must adhere to TracingBlock in order to be managed by the stream
pub trait TracingBlock {
    type Queue: ExtractDeps;

    fn new(
        buffer_size: usize,
        process_id: uuid::Uuid,
        stream_id: uuid::Uuid,
        object_offset: usize,
    ) -> Self;
    fn len_bytes(&self) -> usize;
    fn capacity_bytes(&self) -> usize;
    fn nb_objects(&self) -> usize;
    fn object_offset(&self) -> usize;
    fn events_mut(&mut self) -> &mut Self::Queue;
    fn hint_max_obj_size(&self) -> usize {
        // blocks with less than this amount of available memory will be considered full
        128
    }
}

impl<Q> TracingBlock for EventBlock<Q>
where
    Q: micromegas_transit::HeterogeneousQueue + ExtractDeps,
{
    type Queue = Q;
    fn new(
        buffer_size: usize,
        process_id: uuid::Uuid,
        stream_id: uuid::Uuid,
        event_offset: usize,
    ) -> Self {
        Self {
            process_id,
            stream_id,
            begin: DualTime::now(),
            events: Q::new(buffer_size),
            end: None,
            event_offset,
            dyn_deps: DynDeps::new(),
        }
    }

    fn len_bytes(&self) -> usize {
        self.events.len_bytes() + self.dyn_deps.len_bytes()
    }

    fn capacity_bytes(&self) -> usize {
        self.events.capacity_bytes()
    }

    fn nb_objects(&self) -> usize {
        self.events.nb_objects()
    }

    fn object_offset(&self) -> usize {
        self.event_offset
    }

    fn events_mut(&mut self) -> &mut Self::Queue {
        &mut self.events
    }
}
