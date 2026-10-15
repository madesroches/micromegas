//! Per-block storage for property sets whose values are not `'static`.
use crate::{
    property_set::{DynProperty, Property, PropertySet, sort_dyn_properties},
    static_string_ref::StaticStringRef,
};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};

/// Owns the dynamic property sets referenced by one block's events.
///
/// Addresses are stable until the arena is dropped (each set and value is boxed), which lets
/// them serve as dependency ids within the block. Entries are never removed or mutated once
/// inserted.
#[derive(Debug, Default)]
pub struct PropertyArena {
    // Boxed so that each set keeps its address, which serves as its dependency id.
    #[allow(clippy::vec_box)]
    sets: Vec<Box<PropertySet>>,
    // The sets' value refs point into these boxes.
    values: Vec<Box<str>>,
    index: HashMap<u64, Vec<u32>>,
    len_bytes: usize,
}

/// A sorted property list together with its content hash.
///
/// The hash is computed when the list is built so that [`PropertyArena::get_or_insert`], which
/// runs under the stream lock, only has to do the lookup.
#[derive(Debug, Clone)]
pub struct MergedProperties<'a> {
    properties: Vec<DynProperty<'a>>,
    hash: u64,
}

impl<'a> std::ops::Deref for MergedProperties<'a> {
    type Target = [DynProperty<'a>];

    fn deref(&self) -> &Self::Target {
        &self.properties
    }
}

/// Combines a static set with dynamic properties into one sorted list.
///
/// A dynamic value replaces a static property of the same name (compared case-insensitively
/// to match `property_get`).
pub fn merge_properties<'a>(
    static_set: &'static PropertySet,
    dyn_properties: &[DynProperty<'a>],
) -> MergedProperties<'a> {
    let mut merged: Vec<DynProperty<'a>> = static_set.dyn_properties().collect();
    for dyn_prop in dyn_properties {
        merged.retain(|p| !p.name.eq_ignore_ascii_case(dyn_prop.name));
        merged.push(*dyn_prop);
    }
    sort_dyn_properties(&mut merged);
    let hash = content_hash(merged.iter().map(|p| (p.name, p.value)));
    MergedProperties {
        properties: merged,
        hash,
    }
}

fn content_hash<'a>(properties: impl Iterator<Item = (&'a str, &'a str)>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (name, value) in properties {
        name.hash(&mut hasher);
        value.hash(&mut hasher);
    }
    hasher.finish()
}

fn same_content(stored: &[Property], properties: &[DynProperty<'_>]) -> bool {
    stored.len() == properties.len()
        && stored
            .iter()
            .zip(properties)
            .all(|(x, y)| x.name.as_str() == y.name && x.value.as_str() == y.value)
}

impl PropertyArena {
    /// Returns the arena-owned set equal to `properties`, inserting a copy (with owned
    /// values) on first sight. A hit allocates nothing.
    ///
    pub fn get_or_insert(&mut self, merged: &MergedProperties<'_>) -> *const PropertySet {
        let hash = merged.hash;
        let properties: &[DynProperty<'_>] = merged;
        if let Some(candidates) = self.index.get(&hash) {
            for &idx in candidates {
                let set = &self.sets[idx as usize];
                if same_content(set.get_properties(), properties) {
                    return &**set as *const PropertySet;
                }
            }
        }
        let mut owned = Vec::with_capacity(properties.len());
        for prop in properties {
            let value: Box<str> = prop.value.into();
            self.len_bytes += value.len();
            // SAFETY: the box is pushed into `self.values`, which keeps it alive and
            // unmoved for as long as the set referencing it.
            let value_ref = unsafe { StaticStringRef::from_str_unchecked(&value) };
            self.values.push(value);
            owned.push(Property {
                name: prop.name.into(),
                value: value_ref,
            });
        }
        self.len_bytes +=
            std::mem::size_of::<PropertySet>() + std::mem::size_of_val(owned.as_slice());
        let set = Box::new(PropertySet { properties: owned });
        let ptr = &*set as *const PropertySet;
        let idx = self.sets.len() as u32;
        self.sets.push(set);
        self.index.entry(hash).or_default().push(idx);
        ptr
    }

    /// Approximation of the dependency bytes this arena adds to the encoded block.
    pub fn len_bytes(&self) -> usize {
        self.len_bytes
    }
}
