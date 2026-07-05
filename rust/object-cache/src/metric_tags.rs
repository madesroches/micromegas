//! Bounded, `'static`-safe tag construction for the object cache's
//! dimensioned metrics (`prefix`, `class`).
//!
//! Every dimension used here is a small, closed label set so cardinality
//! stays bounded and every value is a compile-time (or leaked-once-at-
//! startup) `&'static str`, per the tagged-metric contract documented on
//! `micromegas_tracing::property_set::PropertySet` ("the user is expected to
//! manage the cardinality"). Centralizing the `Property`/`PropertySet`
//! construction here keeps the label taxonomy in one place (DRY).

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
