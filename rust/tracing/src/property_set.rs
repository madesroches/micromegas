use crate::static_string_ref::StaticStringRef;
use std::{collections::HashSet, hash::Hash, sync::Mutex};

#[derive(Eq, PartialEq, Hash, Clone)]
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

#[derive(Eq, PartialEq, Hash, Clone)]
pub struct PropertySet {
    properties: Vec<Property>,
}

lazy_static! {
    static ref STORE: Mutex<HashSet<PropertySet>> = Mutex::new(HashSet::new());
}

impl PropertySet {
    pub fn find_or_create(properties: Vec<Property>) -> &'static Self {
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
}
