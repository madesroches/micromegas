use crate::prelude::*;
use crate::property_arena::PropertyArena;

/// Blocks are a batch of events sent as a unit
#[derive(Debug)]
pub struct EventBlock<Q> {
    pub process_id: uuid::Uuid,
    pub stream_id: uuid::Uuid,
    pub begin: DualTime,
    pub events: Q,
    pub end: Option<DualTime>,
    pub event_offset: usize,
    /// Owns the dynamic property sets referenced by `events`.
    ///
    /// Events may hold addresses into this arena. Both live in the same block, which is only
    /// dropped as a whole; `extract()` runs while the block is alive, and arena entries are
    /// never removed or mutated once inserted. Only log and metrics dispatch insert into it.
    pub properties: PropertyArena,
}

impl<Q> EventBlock<Q>
where
    Q: micromegas_transit::HeterogeneousQueue,
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
    type DepsQueue;
    fn extract(&self) -> Self::DepsQueue;
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
            properties: PropertyArena::default(),
        }
    }

    fn len_bytes(&self) -> usize {
        self.events.len_bytes() + self.properties.len_bytes()
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
