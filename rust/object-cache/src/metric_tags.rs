//! Bounded, `'static`-safe tag construction for the object cache's
//! dimensioned metrics (`prefix`, `class`).
//!
//! Every dimension used here is a small, closed label set so cardinality
//! stays bounded and every value is a compile-time (or leaked-once-at-
//! startup) `&'static str`, per the tagged-metric contract documented on
//! `micromegas_tracing::property_set::PropertySet` ("the user is expected to
//! manage the cardinality"). Centralizing the `Property`/`PropertySet`
//! construction here keeps the label taxonomy in one place (DRY).

use std::sync::Arc;

use micromegas_tracing::property_set::{Property, PropertySet};

/// Demand-vs-prefetch `class` dimension values. Kept as plain string
/// constants here (rather than a method on `range_cache`'s private
/// `Priority` enum) so the taxonomy of label strings lives in one place.
pub const CLASS_DEMAND: &str = "demand";
pub const CLASS_PREFETCH: &str = "prefetch";

/// Fallback `prefix` label for a key that matches none of the server's
/// configured allowed prefixes (or when no prefixes were configured at all,
/// e.g. a `RangeCache` built without `RangeCache::with_prefix_labels`).
pub const PREFIX_OTHER: &str = "other";

/// Precomputed, interned `&'static PropertySet`s for one `prefix` label.
///
/// Built once per label at `RangeCache` construction
/// (`RangeCache::with_prefix_labels`) so the hot per-block emission sites in
/// `fetch_blocks` do an array lookup instead of allocating a `Vec` and
/// taking the intern lock on every call.
#[derive(Debug, Clone, Copy)]
pub struct PrefixTags {
    /// The `prefix` label these tags carry, e.g. `"blobs"` or `"other"`.
    pub label: &'static str,
    /// `{prefix}`.
    pub prefix: &'static PropertySet,
    /// `{prefix, class="demand"}`.
    pub prefix_demand: &'static PropertySet,
    /// `{prefix, class="prefetch"}`.
    pub prefix_prefetch: &'static PropertySet,
}

impl PrefixTags {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            prefix: PropertySet::find_or_create(vec![Property::new("prefix", label)]),
            prefix_demand: PropertySet::find_or_create(vec![
                Property::new("prefix", label),
                Property::new("class", CLASS_DEMAND),
            ]),
            prefix_prefetch: PropertySet::find_or_create(vec![
                Property::new("prefix", label),
                Property::new("class", CLASS_PREFETCH),
            ]),
        }
    }

    /// `{prefix, class}` for `class_label`, which must be `CLASS_DEMAND` or
    /// `CLASS_PREFETCH` (any other value falls back to the prefetch tags;
    /// `range_cache.rs`'s callers only ever pass one of the two constants).
    pub fn for_class(&self, class_label: &'static str) -> &'static PropertySet {
        if class_label == CLASS_DEMAND {
            self.prefix_demand
        } else {
            self.prefix_prefetch
        }
    }
}

/// A `{class}`-only tag set, for the run-level latency metrics
/// (`range_cache_fetch_permit_wait_ms`, `range_cache_origin_get_ms`) that
/// aren't dimensioned by `prefix`. Not precomputed like `PrefixTags`: these
/// fire once per coalesced origin run rather than once per block probe, so
/// the per-call intern cost is immaterial.
pub fn class_tags(class_label: &'static str) -> &'static PropertySet {
    PropertySet::find_or_create(vec![Property::new("class", class_label)])
}

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

/// Precomputed, interned `&'static PropertySet`s for one `prefix` label, for
/// the RAM-tier eviction count/age metrics. Parallels `PrefixTags`, but
/// dimensioned by `reason` instead of `class`.
#[derive(Debug, Clone, Copy)]
pub struct EvictionTags {
    /// The `prefix` label these tags carry.
    pub label: &'static str,
    /// `{prefix}` -- used by both the RAM eviction age metric and the disk
    /// read-age metric.
    pub prefix: &'static PropertySet,
    count_evict: &'static PropertySet,
    count_replace: &'static PropertySet,
    count_remove: &'static PropertySet,
    count_clear: &'static PropertySet,
}

impl EvictionTags {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            prefix: PropertySet::find_or_create(vec![Property::new("prefix", label)]),
            count_evict: PropertySet::find_or_create(vec![
                Property::new("prefix", label),
                Property::new("reason", REASON_EVICT),
            ]),
            count_replace: PropertySet::find_or_create(vec![
                Property::new("prefix", label),
                Property::new("reason", REASON_REPLACE),
            ]),
            count_remove: PropertySet::find_or_create(vec![
                Property::new("prefix", label),
                Property::new("reason", REASON_REMOVE),
            ]),
            count_clear: PropertySet::find_or_create(vec![
                Property::new("prefix", label),
                Property::new("reason", REASON_CLEAR),
            ]),
        }
    }

    /// `{prefix, reason}` for `reason`, which must be one of the `REASON_*`
    /// constants; any other value falls back to the evict tags (the listener
    /// only ever passes one of the four constants).
    pub fn count_for(&self, reason: &'static str) -> &'static PropertySet {
        match reason {
            REASON_REPLACE => self.count_replace,
            REASON_REMOVE => self.count_remove,
            REASON_CLEAR => self.count_clear,
            _ => self.count_evict,
        }
    }
}

/// Precomputed table shared (via `Arc`) between the RAM eviction listener and
/// `FoyerBackend::get`, so the key-to-prefix matching rule
/// (`longest_prefix_match`) is not duplicated between the two call sites.
pub struct EvictionTagTable {
    labels: Arc<[&'static str]>,
    tags: Arc<[EvictionTags]>,
    other: EvictionTags,
}

impl EvictionTagTable {
    pub fn new(labels: Arc<[&'static str]>) -> Self {
        let tags: Vec<EvictionTags> = labels
            .iter()
            .map(|&label| EvictionTags::new(label))
            .collect();
        Self {
            labels,
            tags: Arc::from(tags),
            other: EvictionTags::new(PREFIX_OTHER),
        }
    }

    /// The precomputed tags for the `prefix` `key` falls under, resolved by
    /// longest-prefix match against `labels` (the `"other"` fallback tags on
    /// no match).
    pub fn classify(&self, key: &str) -> &EvictionTags {
        match longest_prefix_match(&self.labels, key) {
            Some(i) => &self.tags[i],
            None => &self.other,
        }
    }
}
