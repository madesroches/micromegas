//! Label resolution for the object cache's dimensioned metrics (`prefix`, `class`, `reason`).
//!
//! Every dimension used here is a small, closed label set so cardinality stays bounded, and
//! every value is a compile-time (or leaked-once-at-startup) `&'static str`, per the
//! tagged-metric contract documented on `micromegas_tracing::property_set::PropertySet`
//! ("the user is expected to manage the cardinality"). Call sites attach the labels with
//! `key = static label`; this module only keeps the taxonomy of label strings and resolves a
//! key to its `prefix` label.

use std::sync::Arc;

/// Demand-vs-prefetch `class` dimension values. Kept as plain string
/// constants here (rather than a method on `range_cache`'s private
/// `Priority` enum) so the taxonomy of label strings lives in one place.
pub const CLASS_DEMAND: &str = "demand";
pub const CLASS_PREFETCH: &str = "prefetch";

/// Fallback `prefix` label for a key that matches none of the server's
/// configured allowed prefixes (or when no prefixes were configured at all,
/// e.g. a `RangeCache` built without `RangeCache::with_prefix_labels`).
pub const PREFIX_OTHER: &str = "other";

/// Longest-prefix match of `key` against `labels`, using the same
/// equal-or-`/`-boundary admission rule as `validation::validate_key` (so a
/// label `"blobs"` matches `"blobs"` and `"blobs/x"` but not
/// `"blobs-secret"`). Returns the index of the longest matching label, or
/// `None` if none match -- the caller falls back to `PREFIX_OTHER`.
pub fn longest_prefix_match(labels: &[&'static str], key: &str) -> Option<usize> {
    labels
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            key.starts_with(*p) && (key.len() == p.len() || key.as_bytes()[p.len()] == b'/')
        })
        .max_by_key(|(_, p)| p.len())
        .map(|(i, _)| i)
}

/// RAM-tier eviction `reason` dimension values (`foyer::Event` mapped to a
/// stable label string by `foyer_backend::reason_str`).
pub const REASON_EVICT: &str = "evict";
pub const REASON_REPLACE: &str = "replace";
pub const REASON_REMOVE: &str = "remove";
pub const REASON_CLEAR: &str = "clear";

/// Resolves a key to its `prefix` label, shared (via `Arc`) between the RAM eviction listener and
/// `FoyerBackend::get`, so the key-to-prefix matching rule (`longest_prefix_match`) is not
/// duplicated between the two call sites.
pub struct EvictionTagTable {
    labels: Arc<[&'static str]>,
}

impl EvictionTagTable {
    pub fn new(labels: Arc<[&'static str]>) -> Self {
        Self { labels }
    }

    /// The `prefix` label `key` falls under, resolved by longest-prefix match against `labels`
    /// (`PREFIX_OTHER` on no match).
    pub fn classify(&self, key: &str) -> &'static str {
        match longest_prefix_match(&self.labels, key) {
            Some(i) => self.labels[i],
            None => PREFIX_OTHER,
        }
    }
}
