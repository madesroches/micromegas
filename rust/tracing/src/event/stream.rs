use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::event::TracingBlock;

/// StreamDesc is the metadata associated with an event stream
#[derive(Debug, Serialize, Deserialize)]
pub struct StreamDesc {
    pub stream_id: uuid::Uuid,
    pub process_id: uuid::Uuid,
    pub tags: Vec<String>,
    pub properties: HashMap<String, String>,
}

impl StreamDesc {
    pub fn new(
        process_id: uuid::Uuid,
        tags: &[String],
        properties: HashMap<String, String>,
    ) -> Self {
        let stream_id = uuid::Uuid::new_v4();
        Self {
            stream_id,
            process_id,
            tags: tags.to_vec(),
            properties,
        }
    }
}

/// EventStream are a sequence of blocks sent (or dropped) as they become full
#[derive(Debug)]
pub struct EventStream<Block> {
    stream_desc: Arc<StreamDesc>,
    current_block: Arc<Block>,
    full_threshold: AtomicUsize,
}

impl<Block> EventStream<Block>
where
    Block: TracingBlock,
{
    pub fn new(
        buffer_size: usize,
        process_id: uuid::Uuid,
        tags: &[String],
        properties: HashMap<String, String>,
    ) -> Self {
        let stream_desc = Arc::new(StreamDesc::new(process_id, tags, properties));
        let block = Arc::new(Block::new(
            buffer_size,
            process_id,
            stream_desc.stream_id,
            0,
        ));
        let max_obj_size = block.hint_max_obj_size();
        Self {
            stream_desc,
            current_block: block,
            full_threshold: AtomicUsize::new(buffer_size - max_obj_size),
        }
    }

    pub fn stream_id(&self) -> uuid::Uuid {
        self.stream_desc.stream_id
    }

    pub fn set_full(&mut self) {
        self.full_threshold.store(0, Ordering::Relaxed);
    }

    pub fn replace_block(&mut self, new_block: Arc<Block>) -> Arc<Block> {
        let old_block = self.current_block.clone();
        let max_obj_size = new_block.hint_max_obj_size();
        self.full_threshold
            .store(new_block.capacity_bytes() - max_obj_size, Ordering::Relaxed);
        self.current_block = new_block;
        old_block
    }

    pub fn is_full(&self) -> bool {
        let full_size = self.full_threshold.load(Ordering::Relaxed);
        self.current_block.len_bytes() > full_size
    }

    pub fn is_empty(&self) -> bool {
        self.current_block.len_bytes() == 0
    }

    pub fn get_block_ref(&self) -> &Block {
        &self.current_block
    }

    pub fn get_events_mut(&mut self) -> &mut Block::Queue {
        //get_mut_unchecked should be faster
        Arc::get_mut(&mut self.current_block).unwrap().events_mut()
    }

    pub fn process_id(&self) -> uuid::Uuid {
        self.stream_desc.process_id
    }

    pub fn tags(&self) -> &[String] {
        &self.stream_desc.tags
    }

    pub fn properties(&self) -> &HashMap<String, String> {
        &self.stream_desc.properties
    }
}
