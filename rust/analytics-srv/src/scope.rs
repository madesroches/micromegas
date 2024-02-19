use lgn_telemetry_proto::analytics::ScopeDesc;
use xxhash_rust::const_xxh32::xxh32 as const_xxh32;
pub(crate) type ScopeHashMap = std::collections::HashMap<u32, ScopeDesc>;

pub fn compute_scope_hash(name: &str) -> u32 {
    //todo: add filename
    const_xxh32(name.as_bytes(), 0)
}
