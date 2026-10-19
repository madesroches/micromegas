//! Interned collection of PropertySet instances. Each PropertySet contains properties where the names and the values are statically allocated.
//! The user is expected to manage the cardinality: this holds for `key = static expr` values
//! too, since every distinct combination is interned and never freed.
use crate::{dyn_deps::static_ref_str, static_string_ref::StaticStringRef};
use internment::Intern;
use micromegas_transit::{HeterogeneousQueue, StaticStringDependency, UserDefinedType, prelude::*};
use rustc_hash::FxHashSet;
use std::{
    borrow::Cow,
    hash::Hash,
    sync::{Arc, OnceLock},
};

lazy_static::lazy_static! {
    pub static ref PROPERTY_SET_DEP_TYPE_NAME: Arc<String> = Arc::new("PropertySetDependency".into());
}

#[derive(Debug, Eq, PartialEq, Hash, Clone, TransitReflect)]
pub struct Property {
    pub name: StaticStringRef,
    pub value: StaticStringRef,
}

impl Property {
    pub fn new(name: &'static str, value: &'static str) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// A property whose value is not `'static`. The name is either `'static` (`new`, referenced by
/// address) or block-scoped (`with_dyn_name`, copied into the block like a dynamic value).
/// Names from outside Rust, or otherwise unbounded, belong in `with_dyn_name`.
///
/// For `log!` and metric macros, analytics treats some names as column overrides:
/// `level`/`target` (logs) and `target`/`name`/`unit` (metrics). The macros reject those names
/// as keys, so an override arrives through a `..set` spread, a slice of `DynProperty` built at
/// runtime, or the `imetric_dyn!`/`fmetric_dyn!` macros, which set `name` and `unit`. A dynamic
/// `level` must be a valid level name or the block fails to parse. Interop logs (the `tracing`
/// layer) keep their level and target unchanged.
#[derive(Debug, Clone, Copy)]
pub struct DynProperty<'a> {
    name: &'a str,
    value: &'a str,
    // Private: a `true` flag on a non-`'static` name would make `static_ref_str` read freed memory.
    static_name: bool,
}

impl<'a> DynProperty<'a> {
    pub const fn new(name: &'static str, value: &'a str) -> Self {
        Self {
            name,
            value,
            static_name: true,
        }
    }

    pub const fn with_dyn_name(name: &'a str, value: &'a str) -> Self {
        Self {
            name,
            value,
            static_name: false,
        }
    }

    pub fn name(&self) -> &'a str {
        self.name
    }

    pub fn value(&self) -> &'a str {
        self.value
    }

    #[doc(hidden)]
    pub fn is_static_name(&self) -> bool {
        self.static_name
    }
}

/// Interns the literal properties of one macro callsite on first use. `cell` is a `static`
/// emitted by the macro, so each callsite pays one atomic load afterwards.
///
/// Duplicate names are resolved the way `merge_properties` does: case-insensitive, last wins.
pub fn callsite_set(
    cell: &'static OnceLock<&'static PropertySet>,
    pairs: &'static [(&'static str, &'static str)],
) -> &'static PropertySet {
    cell.get_or_init(|| {
        let mut properties: Vec<Property> = Vec::with_capacity(pairs.len());
        apply_pairs(&mut properties, pairs.iter().copied());
        PropertySet::find_or_create(properties)
    })
}

/// Adds `pairs` in order, each one replacing an earlier property of the same name
/// (case-insensitive), so the last one wins.
pub fn apply_pairs(
    properties: &mut Vec<Property>,
    pairs: impl IntoIterator<Item = (&'static str, &'static str)>,
) {
    for (name, value) in pairs {
        properties.retain(|p| !p.name.as_str().eq_ignore_ascii_case(name));
        properties.push(Property::new(name, value));
    }
}

/// Identity, but only accepts a `&'static str`: a borrowed value passed to a `key = static expr`
/// property is a lifetime error at the call site.
#[inline(always)]
pub fn static_value(v: &'static str) -> &'static str {
    v
}

pub fn dyn_property_get<'a>(properties: &[DynProperty<'a>], name: &str) -> Option<&'a str> {
    properties
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .map(|p| p.value)
}

pub fn property_get<'a>(properties: &'a [Property], name: &str) -> Option<&'a str> {
    properties.iter().find_map(|p| {
        if p.name.as_str().eq_ignore_ascii_case(name) {
            Some(p.value.as_str())
        } else {
            None
        }
    })
}

#[derive(Debug, Clone)]
pub struct PropertySet {
    pub(crate) properties: Vec<Property>,
    // Same order as `properties`. Lets sinks get a slice without a per-call allocation.
    dyn_view: Box<[DynProperty<'static>]>,
}

// Identity is the properties alone, so the cached view never affects interning.
impl PartialEq for PropertySet {
    fn eq(&self, other: &Self) -> bool {
        self.properties == other.properties
    }
}

impl Eq for PropertySet {}

impl Hash for PropertySet {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.properties.hash(state);
    }
}

/// Canonical property order, so that equal sets hash equally.
pub(crate) fn sort_properties(properties: &mut [Property]) {
    properties.sort_by(|a, b| b.name.as_str().cmp(a.name.as_str()));
}

/// Same order as `sort_properties`, so a merged list matches the dynamic set built from it.
#[doc(hidden)]
pub fn cmp_dyn_properties(a: &DynProperty<'_>, b: &DynProperty<'_>) -> std::cmp::Ordering {
    b.name.cmp(a.name)
}

impl PropertySet {
    pub fn find_or_create(mut properties: Vec<Property>) -> &'static Self {
        sort_properties(&mut properties);
        // SAFETY: the refs were built from `&'static str` by `Property::new`, and they don't
        // point into the set, so moving it into `Intern` keeps them valid.
        let dyn_view = properties
            .iter()
            .map(|p| unsafe { DynProperty::new(static_ref_str(&p.name), static_ref_str(&p.value)) })
            .collect();
        let set = PropertySet {
            properties,
            dyn_view,
        };
        Intern::<PropertySet>::new(set).as_ref()
    }

    /// The interned set with no properties, for calls that only carry dynamic ones.
    pub fn empty() -> &'static Self {
        lazy_static::lazy_static! {
            static ref EMPTY: &'static PropertySet = PropertySet::find_or_create(vec![]);
        }
        *EMPTY
    }

    pub fn get_properties(&self) -> &[Property] {
        &self.properties
    }

    pub fn dyn_properties(&'static self) -> &'static [DynProperty<'static>] {
        &self.dyn_view
    }
}

/// Identifies a property set within one block's events: the address of an interned set, or
/// the id of a record in the block's dynamic dependency queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct PropertySetId(pub(crate) u64);

// Dynamic set ids have bit 0 set, which is how they are told apart from interned addresses.
const _: () = assert!(std::mem::align_of::<PropertySet>() >= 2);

impl PropertySetId {
    pub fn interned(set: &'static PropertySet) -> Self {
        Self(set as *const PropertySet as u64)
    }

    /// `None` for the id of a dynamic set, which only its block's `DynDeps` can resolve.
    pub fn as_interned(self) -> Option<&'static PropertySet> {
        if self.0 & 1 != 0 {
            return None;
        }
        // SAFETY: ids with bit 0 clear are only built by `interned`.
        Some(unsafe { &*(self.0 as *const PropertySet) })
    }
}

/// A dependency queue that can receive string records.
pub trait StringDepsQueue: HeterogeneousQueue {
    fn push_string(&mut self, dep: StaticStringDependency);
}

/// A dependency queue that can receive the records of a property set.
pub trait PropertyDepsQueue: StringDepsQueue {
    fn push_set(&mut self, dep: PropertySetDependency);
}

#[derive(Debug)]
pub struct PropertySetDependency {
    pub id: u64,
    pub properties: Cow<'static, [Property]>,
}

/// Emits the dependencies of an interned property set once per block.
pub(crate) fn record_properties<D: PropertyDepsQueue>(
    set: &'static PropertySet,
    recorded_deps: &mut FxHashSet<u64>,
    deps: &mut D,
) {
    let id = PropertySetId::interned(set).0;
    if recorded_deps.insert(id) {
        for prop in set.get_properties() {
            if recorded_deps.insert(prop.name.id()) {
                deps.push_string(prop.name.into_dependency());
            }
            if recorded_deps.insert(prop.value.id()) {
                deps.push_string(prop.value.into_dependency());
            }
        }
        deps.push_set(PropertySetDependency {
            id,
            properties: Cow::Borrowed(set.get_properties()),
        });
    }
}

impl Reflect for PropertySetDependency {
    fn reflect() -> UserDefinedType {
        UserDefinedType {
            name: PROPERTY_SET_DEP_TYPE_NAME.clone(),
            size: 0,
            members: vec![],
            is_reference: false,
            secondary_udts: vec![Property::reflect()],
        }
    }
}

impl InProcSerialize for PropertySetDependency {
    const IN_PROC_SIZE: InProcSize = InProcSize::Dynamic;

    fn get_value_size(&self) -> Option<u32> {
        let header_size: u32 = std::mem::size_of::<u64>() as u32 + // id
			std::mem::size_of::<u32>() as u32; // number of properties
        let container_size: u32 =
            self.properties.len() as u32 * std::mem::size_of::<Property>() as u32;
        let size = header_size + container_size;
        Some(size)
    }

    fn write_value(&self, buffer: &mut Vec<u8>) {
        write_any(buffer, &self.id);
        let nb_properties: u32 = self.properties.len() as u32;
        write_any(buffer, &nb_properties);
        for prop in self.properties.iter() {
            write_any(buffer, prop);
        }
    }

    unsafe fn read_value(mut _window: &[u8]) -> Self {
        // dependencies don't need to be read in the instrumented process
        unimplemented!();
    }
}
