use datafusion::parquet::data_type::AsBytes;
use std::sync::Arc;
use xxhash_rust::xxh32::xxh32;

/// A description of a scope, including its name, location, and hash.
#[derive(Debug)]
pub struct ScopeDesc {
    pub name: Arc<String>,
    pub filename: Arc<String>,
    pub target: Arc<String>,
    pub line: u32,
    pub hash: u32,
}

/// A hash map of scope descriptions, keyed by their hash.
pub type ScopeHashMap = std::collections::HashMap<u32, ScopeDesc>;

impl ScopeDesc {
    pub fn new(name: Arc<String>, filename: Arc<String>, target: Arc<String>, line: u32) -> Self {
        let hash = compute_scope_hash(&name, &filename, &target, line);
        Self {
            name,
            filename,
            target,
            line,
            hash,
        }
    }
}

/// A borrowed scope description handed to block processors per event.
///
/// Borrows the parse arena, so it costs no allocation to construct; the owning
/// [`ScopeDesc`] is materialized only when a scope is first inserted into a
/// long-lived [`ScopeHashMap`] (once per distinct scope, not per event).
#[derive(Debug, Clone, Copy)]
pub struct BorrowedScopeDesc<'a> {
    pub name: &'a str,
    pub filename: &'a str,
    pub target: &'a str,
    pub line: u32,
    pub hash: u32,
}

impl<'a> BorrowedScopeDesc<'a> {
    pub fn new(name: &'a str, filename: &'a str, target: &'a str, line: u32) -> Self {
        let hash = compute_scope_hash(name, filename, target, line);
        Self {
            name,
            filename,
            target,
            line,
            hash,
        }
    }

    /// Materializes an owned `ScopeDesc` (allocates the strings).
    pub fn to_owned(self) -> ScopeDesc {
        ScopeDesc {
            name: Arc::new(self.name.to_owned()),
            filename: Arc::new(self.filename.to_owned()),
            target: Arc::new(self.target.to_owned()),
            line: self.line,
            hash: self.hash,
        }
    }
}

/// Computes the hash of a scope.
pub fn compute_scope_hash(name: &str, filename: &str, target: &str, line: u32) -> u32 {
    let hash_name = xxh32(name.as_bytes(), 0);
    let hash_with_filename = xxh32(filename.as_bytes(), hash_name);
    let hash_with_target = xxh32(target.as_bytes(), hash_with_filename);
    xxh32(line.as_bytes(), hash_with_target)
}
