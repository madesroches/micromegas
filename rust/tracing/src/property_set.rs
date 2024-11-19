//! Interned collection of PropertySet instances. Each PropertySet contains properties where the names and the values are statically allocated.
//! The user is expected to manage the cardinality.
use crate::static_string_ref::StaticStringRef;
use micromegas_transit::{prelude::*, UserDefinedType};
use std::{collections::HashSet, hash::Hash, sync::Mutex};

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

#[derive(Debug, Eq, PartialEq, Hash, Clone)]
pub struct PropertySet {
    properties: Vec<Property>,
}

lazy_static! {
    static ref STORE: Mutex<HashSet<PropertySet>> = Mutex::new(HashSet::new());
}

impl PropertySet {
    pub fn find_or_create(mut properties: Vec<Property>) -> &'static Self {
        // sort properties by name to get the same hash
        properties.sort_by(|a, b| b.name.as_str().cmp(a.name.as_str()));
        let set = PropertySet { properties };
        let mut guard = STORE.lock().unwrap();
        if let Some(found) = guard.get(&set) {
            unsafe { std::mem::transmute::<&PropertySet, &PropertySet>(found) }
        } else {
            guard.insert(set.clone());
            let found = guard.get(&set).unwrap();
            unsafe { std::mem::transmute::<&PropertySet, &PropertySet>(found) }
        }
    }

    pub fn get_properties(&self) -> &[Property] {
        &self.properties
    }
}

#[derive(Debug)]
pub struct PropertySetDependency {
    set: &'static PropertySet,
}

impl PropertySetDependency {
    pub fn new(set: &'static PropertySet) -> Self {
        Self { set }
    }
}

impl Reflect for PropertySetDependency {
    fn reflect() -> UserDefinedType {
        UserDefinedType {
            name: String::from("PropertySetDependency"),
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
            self.set.get_properties().len() as u32 * std::mem::size_of::<Property>() as u32;
        let size = header_size + container_size;
        Some(size)
    }

    fn write_value(&self, buffer: &mut Vec<u8>) {
        let id = self.set as *const _ as u64;
        write_any(buffer, &id);
        let nb_properties: u32 = self.set.get_properties().len() as u32;
        write_any(buffer, &nb_properties);
        for prop in self.set.get_properties() {
            write_any(buffer, prop);
        }
    }

    unsafe fn read_value(mut _window: &[u8]) -> Self {
        // dependencies don't need to be read in the instrumented process
        unimplemented!();
    }
}
