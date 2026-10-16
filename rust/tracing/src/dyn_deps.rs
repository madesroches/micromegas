//! Per-block dependency records for runtime property values, kept in their wire form.
use crate::{
    property_set::{
        DynProperty, Property, PropertyDepsQueue, PropertySet, PropertySetDependency,
        PropertySetId, StringDepsQueue, cmp_dyn_properties,
    },
    static_string_ref::StaticStringRef,
};
use micromegas_transit::{
    HeterogeneousQueue, StaticStringDependency, read_any, string_codec::StringCodec,
};
use rustc_hash::{FxHashMap, FxHashSet};
use std::{borrow::Cow, collections::hash_map::Entry, fmt};
use xxhash_rust::xxh3::{xxh3_64, xxh3_64_with_seed};

/// Keeps dynamic ids out of the range of pointer ids, which the parser requires to be unique
/// within a block. No supported platform hands out addresses with this bit set.
pub const DYN_ID_TAG: u64 = 1 << 63;

const FIRST_INSERT_CAPACITY: usize = 64 * 1024;

// Each record is a type byte and a u32 size, followed by the payload.
const RECORD_HEADER: usize = 1 + std::mem::size_of::<u32>();
// String payload: id, codec, len, then the bytes.
const STRING_HEADER: usize = std::mem::size_of::<u64>() + 1 + std::mem::size_of::<u32>();
// Set payload: id, count, then the properties.
const SET_HEADER: usize = std::mem::size_of::<u64>() + std::mem::size_of::<u32>();

fn dyn_string_id(offset: u32) -> u64 {
    DYN_ID_TAG | (u64::from(offset) << 1)
}

fn dyn_set_id(offset: u32) -> u64 {
    DYN_ID_TAG | (u64::from(offset) << 1) | 1
}

fn dyn_offset(id: u64) -> u32 {
    ((id & !DYN_ID_TAG) >> 1) as u32
}

/// The result of combining a callsite's static set with runtime properties.
pub struct MergedProperties<'a> {
    pub properties: Vec<DynProperty<'a>>,
    // Bit i set: properties[i].value comes from the `&'static` set.
    static_values: u64,
}

impl MergedProperties<'_> {
    fn is_static_value(&self, index: usize) -> bool {
        is_static_value(self.static_values, index)
    }
}

fn is_static_value(mask: u64, index: usize) -> bool {
    index < 64 && (mask >> index) & 1 != 0
}

/// Combines a static set with dynamic properties into one sorted list.
///
/// A dynamic value replaces a static property of the same name (compared case-insensitively
/// to match `property_get`).
#[inline]
pub fn merge_properties<'a>(
    static_set: &'static PropertySet,
    dyn_properties: &[DynProperty<'a>],
) -> MergedProperties<'a> {
    let mut merged: Vec<DynProperty<'a>> = static_set.dyn_properties().to_vec();
    for dyn_prop in dyn_properties {
        merged.retain(|p| !p.name.eq_ignore_ascii_case(dyn_prop.name));
        merged.push(*dyn_prop);
    }
    merged.sort_by(cmp_dyn_properties);
    // A value that is the very memory of a static property's value is `'static` whoever
    // passed it. This avoids carrying a flag through the retain and the sort.
    let static_values = merged
        .iter()
        .take(64)
        .enumerate()
        .fold(0u64, |mask, (i, p)| {
            let is_static = static_set.get_properties().iter().any(|s| {
                s.value.id() == p.value.as_ptr() as u64
                    && s.value.len as usize == p.value.len()
                    && s.name.as_str() == p.name
            });
            mask | (u64::from(is_static) << i)
        });
    MergedProperties {
        properties: merged,
        static_values,
    }
}

fn content_hash<'a>(properties: impl Iterator<Item = (&'a str, &'a str)>) -> u64 {
    properties.fold(0, |h, (name, value)| {
        xxh3_64_with_seed(value.as_bytes(), xxh3_64_with_seed(name.as_bytes(), h))
    })
}

#[inline]
fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap())
}

#[inline]
fn string_at(bytes: &[u8], offset: u32) -> &str {
    let payload = offset as usize + RECORD_HEADER;
    let len = read_u32(bytes, payload + std::mem::size_of::<u64>() + 1) as usize;
    let begin = payload + STRING_HEADER;
    // SAFETY: the bytes were copied from a `&str` by `intern_str`.
    unsafe { std::str::from_utf8_unchecked(&bytes[begin..begin + len]) }
}

#[inline]
fn set_len(bytes: &[u8], offset: u32) -> usize {
    read_u32(
        bytes,
        offset as usize + RECORD_HEADER + std::mem::size_of::<u64>(),
    ) as usize
}

#[inline]
fn set_property(bytes: &[u8], offset: u32, index: usize) -> Property {
    let size = std::mem::size_of::<Property>();
    let begin = offset as usize + RECORD_HEADER + SET_HEADER + index * size;
    let window = &bytes[begin..begin + size];
    // SAFETY: the window holds a `Property` written by `write_any`.
    unsafe { read_any::<Property>(window.as_ptr()) }
}

/// Resolves a ref that was built from a `&'static str`.
///
/// # Safety
/// `r` must have been built from a `&'static str`.
pub(crate) unsafe fn static_ref_str(r: &StaticStringRef) -> &'static str {
    unsafe {
        let slice = std::slice::from_raw_parts(r.ptr as *const u8, r.len as usize);
        std::str::from_utf8_unchecked(slice)
    }
}

/// Records keyed by content hash. A collision is vanishingly rare, so the common case stores
/// one record inline and allocates nothing per key.
struct HashIndex<T> {
    first: FxHashMap<u64, T>,
    collisions: FxHashMap<u64, Vec<T>>,
}

impl<T> HashIndex<T> {
    fn new() -> Self {
        Self {
            first: FxHashMap::default(),
            collisions: FxHashMap::default(),
        }
    }

    fn candidates(&self, hash: u64) -> impl Iterator<Item = &T> {
        let extra = if self.collisions.is_empty() {
            None
        } else {
            self.collisions.get(&hash)
        };
        self.first
            .get(&hash)
            .into_iter()
            .chain(extra.into_iter().flatten())
    }

    fn insert(&mut self, hash: u64, record: T) {
        match self.first.entry(hash) {
            Entry::Vacant(slot) => {
                slot.insert(record);
            }
            Entry::Occupied(_) => self.collisions.entry(hash).or_default().push(record),
        }
    }

    fn iter(&self) -> impl Iterator<Item = &T> {
        self.first
            .values()
            .chain(self.collisions.values().flatten())
    }
}

#[derive(Debug)]
struct DynSet {
    offset: u32,
    // The mask of the `MergedProperties` the record was built with.
    static_values: u64,
}

/// The dependency records of one block that only exist at run time.
///
/// Records are serialized when the event is pushed, with ids that are scoped to the block, and
/// appended as-is after the static dependencies when the block is extracted. A record only
/// references records pushed before it, or `'static` data.
pub struct DynDeps<D> {
    queue: D,
    // offsets of string records
    strings: HashIndex<u32>,
    sets: HashIndex<DynSet>,
}

impl<D> fmt::Debug for DynDeps<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DynDeps").finish_non_exhaustive()
    }
}

impl<D: HeterogeneousQueue> Default for DynDeps<D> {
    fn default() -> Self {
        Self::new()
    }
}

impl<D: HeterogeneousQueue> DynDeps<D> {
    /// Allocates nothing until the first insert.
    pub fn new() -> Self {
        Self {
            queue: D::new(0),
            strings: HashIndex::new(),
            sets: HashIndex::new(),
        }
    }

    /// Exact size of the dependency bytes this adds to the encoded block.
    pub fn len_bytes(&self) -> usize {
        self.queue.len_bytes()
    }

    pub fn queue(&self) -> &D {
        &self.queue
    }

    fn reserve_on_first_insert(&mut self) {
        if self.queue.capacity_bytes() == 0 {
            self.queue = D::new(FIRST_INSERT_CAPACITY);
        }
    }

    /// Decodes a set for in-process readers.
    pub fn set_properties(&self, id: PropertySetId) -> Vec<(&str, &str)> {
        if let Some(set) = id.as_interned() {
            return set
                .get_properties()
                .iter()
                .map(|p| (p.name.as_str(), p.value.as_str()))
                .collect();
        }
        let offset = dyn_offset(id.0);
        let bytes = self.queue.as_bytes();
        let static_values = self
            .sets
            .iter()
            .find(|s| s.offset == offset)
            .expect("unknown dynamic set id")
            .static_values;
        (0..set_len(bytes, offset))
            .map(|i| {
                let prop = set_property(bytes, offset, i);
                // SAFETY: names and masked values were built from `&'static str`s.
                let name = unsafe { static_ref_str(&prop.name) };
                let value = if is_static_value(static_values, i) {
                    unsafe { static_ref_str(&prop.value) }
                } else {
                    string_at(bytes, dyn_offset(prop.value.ptr))
                };
                (name, value)
            })
            .collect()
    }
}

impl<D: StringDepsQueue> DynDeps<D> {
    /// Returns the id of the string record holding `value`, pushing one on first sight.
    pub fn intern_str(&mut self, value: &str) -> u64 {
        self.reserve_on_first_insert();
        let hash = xxh3_64(value.as_bytes());
        let bytes = self.queue.as_bytes();
        for &offset in self.strings.candidates(hash) {
            if string_at(bytes, offset) == value {
                return dyn_string_id(offset);
            }
        }
        let offset = u32::try_from(self.queue.len_bytes()).expect("dynamic queue exceeds 4 GiB");
        let id = dyn_string_id(offset);
        self.queue.push_string(StaticStringDependency {
            id,
            codec: StringCodec::Utf8,
            len: value.len() as u32,
            ptr: value.as_ptr(),
        });
        self.strings.insert(hash, offset);
        id
    }
}

impl<D: PropertyDepsQueue> DynDeps<D> {
    /// Returns the id of the set record equal to `merged`, pushing it (and the string records
    /// it needs) on first sight. A hit allocates nothing.
    #[inline]
    pub fn intern_set(&mut self, merged: &MergedProperties<'_>) -> PropertySetId {
        self.reserve_on_first_insert();
        let hash = content_hash(merged.properties.iter().map(|p| (p.name, p.value)));
        let bytes = self.queue.as_bytes();
        for candidate in self.sets.candidates(hash) {
            if Self::same_content(bytes, candidate, merged) {
                return PropertySetId(dyn_set_id(candidate.offset));
            }
        }
        let mut properties = Vec::with_capacity(merged.properties.len());
        for (i, prop) in merged.properties.iter().enumerate() {
            let value = if merged.is_static_value(i) {
                // SAFETY: the mask marks values that came from a `&'static` set.
                unsafe { StaticStringRef::from_str_unchecked(prop.value) }
            } else {
                // Only ever serialized: the ptr is the id of the string record.
                StaticStringRef {
                    ptr: self.intern_str(prop.value),
                    len: prop.value.len() as u32,
                    codec: StringCodec::Utf8,
                }
            };
            properties.push(Property {
                name: prop.name.into(),
                value,
            });
        }
        let offset = u32::try_from(self.queue.len_bytes()).expect("dynamic queue exceeds 4 GiB");
        let id = dyn_set_id(offset);
        self.queue.push_set(PropertySetDependency {
            id,
            properties: Cow::Owned(properties),
        });
        self.sets.insert(
            hash,
            DynSet {
                offset,
                static_values: merged.static_values,
            },
        );
        PropertySetId(id)
    }

    #[inline]
    fn same_content(bytes: &[u8], candidate: &DynSet, merged: &MergedProperties<'_>) -> bool {
        if set_len(bytes, candidate.offset) != merged.properties.len() {
            return false;
        }
        merged.properties.iter().enumerate().all(|(i, wanted)| {
            let prop = set_property(bytes, candidate.offset, i);
            // SAFETY: names and masked values were built from `&'static str`s.
            if unsafe { static_ref_str(&prop.name) } != wanted.name {
                return false;
            }
            if is_static_value(candidate.static_values, i) {
                unsafe { static_ref_str(&prop.value) == wanted.value }
            } else {
                string_at(bytes, dyn_offset(prop.value.ptr)) == wanted.value
            }
        })
    }

    /// Emits the `'static` names and values that dynamic sets reference, once each.
    pub(crate) fn record_static_refs(&self, recorded_deps: &mut FxHashSet<u64>, deps: &mut D) {
        let bytes = self.queue.as_bytes();
        for set in self.sets.iter() {
            for i in 0..set_len(bytes, set.offset) {
                let prop = set_property(bytes, set.offset, i);
                if recorded_deps.insert(prop.name.id()) {
                    deps.push_string(prop.name.into_dependency());
                }
                if is_static_value(set.static_values, i) && recorded_deps.insert(prop.value.id()) {
                    deps.push_string(prop.value.into_dependency());
                }
            }
        }
    }
}
