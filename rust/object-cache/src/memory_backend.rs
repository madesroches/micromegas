use async_trait::async_trait;
use bytes::Bytes;
use std::collections::HashMap;
use std::sync::Mutex;

use super::backend::{FillHint, RangeCacheBackend};

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
    async fn get(&self, key: &str, _expected_len: u64) -> Option<Bytes> {
        self.data
            .lock()
            .expect("memory backend lock")
            .get(key)
            .cloned()
    }

    async fn put(&self, key: String, value: Bytes, _hint: FillHint) {
        self.data
            .lock()
            .expect("memory backend lock")
            .insert(key, value);
    }
}
