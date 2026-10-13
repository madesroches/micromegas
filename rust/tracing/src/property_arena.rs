//! Per-block storage for property sets whose values are not `'static`.
use crate::{
    dispatch::DynProperty,
    property_set::{Property, PropertySet, sort_properties},
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

/// Combines a static set with dynamic properties into one sorted list.
///
/// A dynamic value replaces a static property of the same name (compared case-insensitively
/// to match `property_get`). The refs of the dynamic entries point at the caller's strings
/// and are only valid while those strings are.
///
/// # Safety
/// The strings referenced by `dyn_properties` must outlive every use of the returned list, and
/// the list must never be passed to `PropertySet::find_or_create`, which would intern the refs.
pub unsafe fn merge_properties(
    static_set: &PropertySet,
    dyn_properties: &[DynProperty<'_>],
) -> Vec<Property> {
    let mut merged: Vec<Property> = static_set.get_properties().to_vec();
    for dyn_prop in dyn_properties {
        merged.retain(|p| !p.name.as_str().eq_ignore_ascii_case(dyn_prop.name));
        merged.push(Property {
            name: dyn_prop.name.into(),
            // SAFETY: the merged list is only used for the duration of the dispatch call,
            // and the arena copies the value before keeping it.
            value: unsafe { StaticStringRef::from_str_unchecked(dyn_prop.value) },
        });
    }
    sort_properties(&mut merged);
    merged
}

fn content_hash(properties: &[Property]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for prop in properties {
        prop.name.as_str().hash(&mut hasher);
        prop.value.as_str().hash(&mut hasher);
    }
    hasher.finish()
}

fn same_content(a: &[Property], b: &[Property]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.name.as_str() == y.name.as_str() && x.value.as_str() == y.value.as_str()
        })
}

impl PropertyArena {
    /// Returns the arena-owned set equal to `properties`, inserting a copy (with owned
    /// values) on first sight. A hit allocates nothing.
    ///
    /// `properties` must already be sorted the way [`PropertySet::find_or_create`] sorts them
    /// (see `merge_properties`), otherwise equal sets may be stored twice.
    pub fn get_or_insert(&mut self, properties: &[Property]) -> *const PropertySet {
        let hash = content_hash(properties);
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
            let value: Box<str> = prop.value.as_str().into();
            self.len_bytes += value.len();
            // SAFETY: the box is pushed into `self.values`, which keeps it alive and
            // unmoved for as long as the set referencing it.
            let value_ref = unsafe { StaticStringRef::from_str_unchecked(&value) };
            self.values.push(value);
            owned.push(Property {
                name: prop.name.clone(),
                value: value_ref,
            });
        }
        self.len_bytes += std::mem::size_of::<PropertySet>() + std::mem::size_of_val(properties);
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
