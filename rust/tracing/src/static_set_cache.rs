//! Per-callsite cache behind `key = static expr` properties.
use crate::property_set::{Property, PropertySet, apply_pairs};
use rustc_hash::FxBuildHasher;
use std::{
    collections::HashMap,
    sync::{PoisonError, RwLock},
};

// The base set's address, then `(ptr, len)` of every value. Names are fixed per callsite, so
// they stay out of the key; `len` keeps two substrings that share a start address apart.
type Key<const M: usize> = (usize, [(usize, usize); M]);

/// Interned sets of one callsite, one per distinct tuple of values. Entries are never removed:
/// the cardinality contract of `PropertySet` applies.
pub struct StaticSetCache<const M: usize> {
    map: RwLock<HashMap<Key<M>, &'static PropertySet, FxBuildHasher>>,
}

impl<const M: usize> Default for StaticSetCache<M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const M: usize> StaticSetCache<M> {
    pub const fn new() -> Self {
        Self {
            map: RwLock::new(HashMap::with_hasher(FxBuildHasher)),
        }
    }

    /// The set made of `base` with `pairs` applied in order (case-insensitive, last wins).
    pub fn get(
        &self,
        base: &'static PropertySet,
        pairs: [(&'static str, &'static str); M],
    ) -> &'static PropertySet {
        let key: Key<M> = (
            base as *const PropertySet as usize,
            pairs.map(|(_, v)| (v.as_ptr() as usize, v.len())),
        );
        // Poisoning is recovered from: the map only holds `&'static` values, and an insert
        // can't be observed half-done.
        let hit = self
            .map
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .copied();
        if let Some(set) = hit {
            return set;
        }
        let mut properties: Vec<Property> = base.get_properties().to_vec();
        apply_pairs(&mut properties, pairs);
        // Interned outside any lock of ours. Two threads missing the same key intern the same
        // set, and the map keeps one entry.
        let set = PropertySet::find_or_create(properties);
        self.map
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key)
            .or_insert(set)
    }
}
