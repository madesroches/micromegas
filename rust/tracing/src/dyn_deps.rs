//! Per-block dependency records for runtime property values, kept in their wire form.
use crate::{
    property_set::{
        DynProperty, Property, PropertyDepsQueue, PropertySet, PropertySetDependency,
        PropertySetId, StringDepsQueue, cmp_dyn_properties,
    },
    static_string_ref::{BlockStringRef, StaticStringRef},
};
use micromegas_transit::{
    HeterogeneousQueue, StaticStringDependency, read_any, string_codec::StringCodec,
};
use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
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

/// Callsites rarely carry more than a handful of properties, so this keeps the merge off the heap.
const INLINE_PROPERTIES: usize = 8;

/// The result of combining a callsite's static set with runtime properties.
pub struct MergedProperties<'a> {
    properties: SmallVec<[DynProperty<'a>; INLINE_PROPERTIES]>,
    // Bit i set: properties[i].value comes from the `&'static` set.
    static_values: u64,
    // Bit i set: properties[i].name is `'static`.
    static_names: u64,
}

impl<'a> MergedProperties<'a> {
    pub fn properties(&self) -> &[DynProperty<'a>] {
        &self.properties
    }

    #[doc(hidden)]
    pub fn is_static_value(&self, index: usize) -> bool {
        is_static_bit(self.static_values, index)
    }

    #[doc(hidden)]
    pub fn is_static_name(&self, index: usize) -> bool {
        is_static_bit(self.static_names, index)
    }
}

fn is_static_bit(mask: u64, index: usize) -> bool {
    index < 64 && (mask >> index) & 1 != 0
}

/// Combines a static set with dynamic properties into one sorted list.
///
/// A dynamic value replaces a static property of the same name (compared case-insensitively
/// to match `property_get`). Among dynamic properties, the last of a name wins.
#[inline]
pub fn merge_properties<'a>(
    static_set: &'static PropertySet,
    dyn_properties: &[DynProperty<'a>],
) -> MergedProperties<'a> {
    let mut properties: SmallVec<[DynProperty<'a>; INLINE_PROPERTIES]> = SmallVec::new();
    for dyn_prop in dyn_properties {
        properties.retain(|p| !p.name().eq_ignore_ascii_case(dyn_prop.name()));
        properties.push(*dyn_prop);
    }
    properties.sort_by(cmp_dyn_properties);

    let statics = static_set.dyn_properties();
    if statics.is_empty() {
        // The `_dyn` metric macros and interop logs land here: nothing to merge. Building in
        // place keeps the inline buffer from being copied on the way out.
        let static_names = properties
            .iter()
            .take(64)
            .enumerate()
            .fold(0u64, |names, (i, p)| {
                names | (u64::from(p.is_static_name()) << i)
            });
        return MergedProperties {
            properties,
            static_values: 0,
            static_names,
        };
    }
    let deduped = std::mem::take(&mut properties);
    // The static view is already in canonical order, so this is a merge of two sorted lists.
    // Static entries keep their relative order, as a stable sort of the concatenation would.
    let mut static_values = 0u64;
    let mut static_names = 0u64;
    let mut dynamic = deduped.iter().peekable();
    let mut statics = statics
        .iter()
        .filter(|s| {
            !deduped
                .iter()
                .any(|d| d.name().eq_ignore_ascii_case(s.name()))
        })
        .peekable();
    loop {
        let from_static = match (statics.peek(), dynamic.peek()) {
            (None, None) => break,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            // An exact name match was filtered out, so the heads never tie.
            (Some(s), Some(d)) => cmp_dyn_properties(s, d).is_le(),
        };
        let prop = if from_static {
            *statics.next().unwrap()
        } else {
            *dynamic.next().unwrap()
        };
        let i = properties.len();
        if i < 64 {
            static_values |= u64::from(from_static) << i;
            static_names |= u64::from(prop.is_static_name()) << i;
        }
        properties.push(prop);
    }
    MergedProperties {
        properties,
        static_values,
        static_names,
    }
}

#[doc(hidden)]
pub fn content_hash<'a>(properties: impl Iterator<Item = (&'a str, &'a str)>) -> u64 {
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
    // The masks of the `MergedProperties` the record was built with.
    static_values: u64,
    static_names: u64,
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
        let set = self
            .sets
            .iter()
            .find(|s| s.offset == offset)
            .expect("unknown dynamic set id");
        (0..set_len(bytes, offset))
            .map(|i| {
                let prop = set_property(bytes, offset, i);
                (
                    ref_str(bytes, &prop.name, set.static_names, i),
                    ref_str(bytes, &prop.value, set.static_values, i),
                )
            })
            .collect()
    }

    /// The text of a string record written by `intern_str`.
    pub fn str(&self, id: u64) -> &str {
        string_at(self.queue.as_bytes(), dyn_offset(id))
    }
}

/// Resolves a name or value of a dynamic set: a `'static` string when its bit is set in
/// `mask`, a string record of the block otherwise.
#[inline]
fn ref_str<'b>(bytes: &'b [u8], r: &StaticStringRef, mask: u64, index: usize) -> &'b str {
    if is_static_bit(mask, index) {
        // SAFETY: masked refs were built from `&'static str`s.
        unsafe { static_ref_str(r) }
    } else {
        string_at(bytes, dyn_offset(r.ptr))
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

    /// Like `intern_str`, as the reference an event stores.
    pub fn intern_str_ref(&mut self, value: &str) -> BlockStringRef {
        BlockStringRef {
            id: self.intern_str(value),
            len: value.len() as u32,
            codec: StringCodec::Utf8,
        }
    }
}

impl<D: PropertyDepsQueue> DynDeps<D> {
    /// Returns the id of the set record equal to `merged`, pushing it (and the string records
    /// it needs) on first sight. A hit allocates nothing.
    #[inline]
    pub fn intern_set(&mut self, merged: &MergedProperties<'_>) -> PropertySetId {
        self.reserve_on_first_insert();
        let hash = content_hash(merged.properties.iter().map(|p| (p.name(), p.value())));
        let bytes = self.queue.as_bytes();
        for candidate in self.sets.candidates(hash) {
            if Self::same_content(bytes, candidate, merged) {
                return PropertySetId(dyn_set_id(candidate.offset));
            }
        }
        let mut properties = Vec::with_capacity(merged.properties.len());
        for (i, prop) in merged.properties.iter().enumerate() {
            // Only ever serialized: the ptr of a dynamic ref is the id of the string record.
            let name = if merged.is_static_name(i) {
                // SAFETY: the mask marks names that were built from a `&'static str`.
                unsafe { StaticStringRef::from_str_unchecked(prop.name()) }
            } else {
                StaticStringRef {
                    ptr: self.intern_str(prop.name()),
                    len: prop.name().len() as u32,
                    codec: StringCodec::Utf8,
                }
            };
            let value = if merged.is_static_value(i) {
                // SAFETY: the mask marks values that came from a `&'static` set.
                unsafe { StaticStringRef::from_str_unchecked(prop.value()) }
            } else {
                StaticStringRef {
                    ptr: self.intern_str(prop.value()),
                    len: prop.value().len() as u32,
                    codec: StringCodec::Utf8,
                }
            };
            properties.push(Property { name, value });
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
                static_names: merged.static_names,
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
            ref_str(bytes, &prop.name, candidate.static_names, i) == wanted.name()
                && ref_str(bytes, &prop.value, candidate.static_values, i) == wanted.value()
        })
    }

    /// Emits the `'static` names and values that dynamic sets reference, once each. Block-scoped
    /// ones are already in the queue.
    pub(crate) fn record_static_refs(&self, recorded_deps: &mut FxHashSet<u64>, deps: &mut D) {
        let bytes = self.queue.as_bytes();
        for set in self.sets.iter() {
            for i in 0..set_len(bytes, set.offset) {
                let prop = set_property(bytes, set.offset, i);
                if is_static_bit(set.static_names, i) && recorded_deps.insert(prop.name.id()) {
                    deps.push_string(prop.name.into_dependency());
                }
                if is_static_bit(set.static_values, i) && recorded_deps.insert(prop.value.id()) {
                    deps.push_string(prop.value.into_dependency());
                }
            }
        }
    }
}
