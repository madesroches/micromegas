use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::event::TracingBlock;

#[derive(Debug)]
pub struct EventStream<Block> {
    stream_id: String,
    process_id: String,
    current_block: Arc<Block>,
    full_threshold: AtomicUsize,
    tags: Vec<String>,
    properties: HashMap<String, String>,
}

impl<Block> EventStream<Block>
where
    Block: TracingBlock,
{
    pub fn new(
        buffer_size: usize,
        process_id: String,
        tags: &[String],
        properties: HashMap<String, String>,
    ) -> Self {
        let stream_id = uuid::Uuid::new_v4().to_string();
        let block = Arc::new(Block::new(buffer_size, stream_id.clone()));
        let max_obj_size = block.hint_max_obj_size();
        Self {
            current_block: block,
            full_threshold: AtomicUsize::new(buffer_size - max_obj_size),
            stream_id,
            process_id,
            tags: tags.to_vec(),
            properties,
        }
    }

    pub fn stream_id(&self) -> &str {
        self.stream_id.as_str()
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

    pub fn get_events_mut(&mut self) -> &mut Block::Queue {
        //get_mut_unchecked should be faster
        Arc::get_mut(&mut self.current_block).unwrap().events_mut()
    }

    pub fn process_id(&self) -> &str {
        &self.process_id
    }

    pub fn tags(&self) -> &[String] {
        &self.tags
    }

    pub fn properties(&self) -> &HashMap<String, String> {
        &self.properties
    }
}
