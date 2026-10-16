//! Interned collection of PropertySet instances. Each PropertySet contains properties where the names and the values are statically allocated.
//! The user is expected to manage the cardinality.
use crate::static_string_ref::StaticStringRef;
use internment::Intern;
use micromegas_transit::{StaticStringDependency, UserDefinedType, prelude::*};
use rustc_hash::FxHashSet;
use std::{
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

/// A property whose value is not `'static`. Names stay `'static`: name cardinality must remain
/// bounded.
///
/// For `log!` and metric macros, analytics treats some names as column overrides:
/// `level`/`target` (logs) and `target`/`name`/`unit` (metrics). The macros reject those names
/// as keys, so an override only arrives through a `..set` spread or a slice of `DynProperty`
/// built at runtime. A dynamic `level` must be a valid level name or the block fails to parse.
/// Interop logs (the `tracing` layer) keep their level and target unchanged.
#[derive(Debug, Clone, Copy)]
pub struct DynProperty<'a> {
    pub name: &'static str,
    pub value: &'a str,
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
        for (name, value) in pairs {
            properties.retain(|p| !p.name.as_str().eq_ignore_ascii_case(name));
            properties.push(Property::new(name, value));
        }
        PropertySet::find_or_create(properties)
    })
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

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct PropertySet {
    pub(crate) properties: Vec<Property>,
}

/// Canonical property order, so that equal sets hash equally.
pub(crate) fn sort_properties(properties: &mut [Property]) {
    properties.sort_by(|a, b| b.name.as_str().cmp(a.name.as_str()));
}

/// Same order as `sort_properties`, so a merged list matches the arena set built from it.
pub(crate) fn sort_dyn_properties(properties: &mut [DynProperty<'_>]) {
    properties.sort_by(|a, b| b.name.cmp(a.name));
}

impl PropertySet {
    pub fn find_or_create(mut properties: Vec<Property>) -> &'static Self {
        sort_properties(&mut properties);
        let set = PropertySet { properties };
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

    pub fn dyn_properties(&'static self) -> impl Iterator<Item = DynProperty<'static>> {
        self.properties.iter().map(|p| DynProperty {
            name: p.name.as_str(),
            value: p.value.as_str(),
        })
    }
}

#[derive(Debug)]
pub struct PropertySetDependency {
    set: *const PropertySet,
}

impl PropertySetDependency {
    /// `set` must stay valid until the dependency is written: interned sets always are,
    /// arena sets live as long as the block that `extract` runs on.
    pub(crate) fn new(set: *const PropertySet) -> Self {
        Self { set }
    }

    fn set(&self) -> &PropertySet {
        // SAFETY: see `new`.
        unsafe { &*self.set }
    }
}

/// Emits the dependencies of a property set once per block.
///
/// # Safety
/// `set` must be an interned set or belong to the arena of the block being extracted.
pub(crate) unsafe fn record_properties<D>(
    set: *const PropertySet,
    recorded_deps: &mut FxHashSet<u64>,
    deps: &mut D,
    push_string: fn(&mut D, StaticStringDependency),
    push_set: fn(&mut D, PropertySetDependency),
) {
    let id = set as u64;
    if recorded_deps.insert(id) {
        // SAFETY: guaranteed by the caller.
        let set_ref = unsafe { &*set };
        for prop in set_ref.get_properties() {
            if recorded_deps.insert(prop.name.id()) {
                push_string(deps, prop.name.into_dependency());
            }
            if recorded_deps.insert(prop.value.id()) {
                push_string(deps, prop.value.into_dependency());
            }
        }
        push_set(deps, PropertySetDependency::new(set));
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
            self.set().get_properties().len() as u32 * std::mem::size_of::<Property>() as u32;
        let size = header_size + container_size;
        Some(size)
    }

    fn write_value(&self, buffer: &mut Vec<u8>) {
        let id = self.set as u64;
        write_any(buffer, &id);
        let nb_properties: u32 = self.set().get_properties().len() as u32;
        write_any(buffer, &nb_properties);
        for prop in self.set().get_properties() {
            write_any(buffer, prop);
        }
    }

    unsafe fn read_value(mut _window: &[u8]) -> Self {
        // dependencies don't need to be read in the instrumented process
        unimplemented!();
    }
}
