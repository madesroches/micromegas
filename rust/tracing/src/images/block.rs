use super::ImageEvent;
use crate::dyn_deps::DynDeps;
use crate::event::{EventBlock, EventStream, ExtractDeps};
use micromegas_transit::prelude::*;

declare_queue_struct!(
    struct ImageMsgQueue<ImageEvent> {}
);
declare_queue_struct!(
    struct ImageDepsQueue {}
);

impl ExtractDeps for ImageMsgQueue {
    type DepsQueue = ImageDepsQueue;

    fn extract(&self, _dyn_deps: &DynDeps<Self::DepsQueue>) -> Self::DepsQueue {
        ImageDepsQueue::new(0)
    }
}

pub type ImageBlock = EventBlock<ImageMsgQueue>;
pub type ImageStream = EventStream<ImageBlock>;
