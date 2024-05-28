use std::{collections::HashSet, sync::Mutex};

lazy_static! {
    static ref LOCKED_HASH: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
}

pub fn intern_string(input: &str) -> &'static str {
    let mut lock = LOCKED_HASH.lock().unwrap();
    if let Some(val) = lock.get(input) {
        unsafe { std::mem::transmute::<&str, &'static str>(val) }
    } else {
        lock.insert(input.to_string());
        let interned = lock.get(input).unwrap();
        unsafe { std::mem::transmute::<&str, &'static str>(interned) }
    }
}
