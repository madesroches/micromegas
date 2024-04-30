use datafusion::parquet::data_type::AsBytes;
use std::sync::Arc;
use xxhash_rust::xxh32::xxh32;

#[derive(Debug)]
pub struct ScopeDesc {
    pub name: Arc<String>,
    pub filename: Arc<String>,
    pub line: u32,
    pub hash: u32,
}

pub type ScopeHashMap = std::collections::HashMap<u32, ScopeDesc>;

impl ScopeDesc {
    pub fn new(name: Arc<String>, filename: Arc<String>, line: u32) -> Self {
        let hash = compute_scope_hash(&name, &filename, line);
        Self {
            name,
            filename,
            line,
            hash,
        }
    }
}

pub fn compute_scope_hash(name: &str, filename: &str, line: u32) -> u32 {
    let hash_name = xxh32(name.as_bytes(), 0);
    let hash_with_filename = xxh32(filename.as_bytes(), hash_name);
    let hash_with_line = xxh32(line.as_bytes(), hash_with_filename);
    hash_with_line
}
