use async_trait::async_trait;
use bytes::Bytes;
use std::collections::HashMap;
use std::sync::Mutex;

use super::backend::RangeCacheBackend;

pub struct MemoryBackend {
    data: Mutex<HashMap<String, Bytes>>,
}

impl MemoryBackend {
    pub fn new() -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for MemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RangeCacheBackend for MemoryBackend {
    async fn get(&self, key: &str) -> Option<Bytes> {
        self.data
            .lock()
            .expect("memory backend lock")
            .get(key)
            .cloned()
    }

    async fn put(&self, key: String, value: Bytes) {
        self.data
            .lock()
            .expect("memory backend lock")
            .insert(key, value);
    }
}
