use crate::DualTime;

#[derive(Debug)]
pub struct EventBlock<Q> {
    pub process_id: String,
    pub stream_id: String,
    pub begin: DualTime,
    pub events: Q,
    pub end: Option<DualTime>,
}

impl<Q> EventBlock<Q>
where
    Q: micromegas_transit::HeterogeneousQueue,
{
    pub fn close(&mut self) {
        self.end = Some(DualTime::now());
    }
}

pub trait ExtractDeps {
    type DepsQueue;
    fn extract(&self) -> Self::DepsQueue;
}

pub trait TracingBlock {
    type Queue: ExtractDeps;

    fn new(buffer_size: usize, process_id: String, stream_id: String) -> Self;
    fn len_bytes(&self) -> usize;
    fn capacity_bytes(&self) -> usize;
    fn nb_objects(&self) -> usize;
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
    fn new(buffer_size: usize, process_id: String, stream_id: String) -> Self {
        Self {
            process_id,
            stream_id,
            begin: DualTime::now(),
            events: Q::new(buffer_size),
            end: None,
        }
    }

    fn len_bytes(&self) -> usize {
        self.events.len_bytes()
    }

    fn capacity_bytes(&self) -> usize {
        self.events.capacity_bytes()
    }

    fn nb_objects(&self) -> usize {
        self.events.nb_objects()
    }

    fn events_mut(&mut self) -> &mut Self::Queue {
        &mut self.events
    }
}
