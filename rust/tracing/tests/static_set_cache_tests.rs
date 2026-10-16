use micromegas_tracing::{
    property_set::{Property, PropertySet},
    static_set_cache::StaticSetCache,
};
use std::sync::Barrier;

fn props(set: &PropertySet) -> Vec<(&str, &str)> {
    let mut v: Vec<_> = set
        .get_properties()
        .iter()
        .map(|p| (p.name.as_str(), p.value.as_str()))
        .collect();
    v.sort();
    v
}

#[test]
fn same_values_return_the_same_set() {
    static CACHE: StaticSetCache<1> = StaticSetCache::new();
    let a = CACHE.get(PropertySet::empty(), [("k", "v")]);
    let b = CACHE.get(PropertySet::empty(), [("k", "v")]);
    assert!(std::ptr::eq(a, b));
}

#[test]
fn different_values_return_different_sets() {
    static CACHE: StaticSetCache<2> = StaticSetCache::new();
    let a = CACHE.get(PropertySet::empty(), [("k", "v1"), ("l", "x")]);
    let b = CACHE.get(PropertySet::empty(), [("k", "v2"), ("l", "x")]);
    assert!(!std::ptr::eq(a, b));
    assert_eq!(props(a), [("k", "v1"), ("l", "x")]);
    assert_eq!(props(b), [("k", "v2"), ("l", "x")]);
}

// Property identity is the address of the string, so equal content at two addresses is two
// interned sets, each cached under its own key.
#[test]
fn equal_content_at_two_addresses_is_cached_per_address() {
    static CACHE: StaticSetCache<1> = StaticSetCache::new();
    let first: &'static str = Box::leak(String::from("same").into_boxed_str());
    let second: &'static str = Box::leak(String::from("same").into_boxed_str());
    let a = CACHE.get(PropertySet::empty(), [("k", first)]);
    let b = CACHE.get(PropertySet::empty(), [("k", second)]);
    assert_eq!(props(a), props(b));
    assert!(std::ptr::eq(
        a,
        CACHE.get(PropertySet::empty(), [("k", first)])
    ));
    assert!(std::ptr::eq(
        b,
        CACHE.get(PropertySet::empty(), [("k", second)])
    ));
}

#[test]
fn substrings_sharing_a_start_address_are_distinct() {
    static CACHE: StaticSetCache<1> = StaticSetCache::new();
    let full: &'static str = "abcdef";
    let a = CACHE.get(PropertySet::empty(), [("k", full)]);
    let b = CACHE.get(PropertySet::empty(), [("k", &full[..3])]);
    assert_eq!(props(a), [("k", "abcdef")]);
    assert_eq!(props(b), [("k", "abc")]);
}

#[test]
fn base_set_is_part_of_the_key_and_pairs_override_it() {
    static CACHE: StaticSetCache<1> = StaticSetCache::new();
    let base1 = PropertySet::find_or_create(vec![
        Property::new("region", "eu"),
        Property::new("k", "base"),
    ]);
    let base2 = PropertySet::find_or_create(vec![Property::new("region", "us")]);
    let a = CACHE.get(base1, [("k", "v")]);
    let b = CACHE.get(base2, [("k", "v")]);
    assert!(!std::ptr::eq(a, b));
    assert_eq!(props(a), [("k", "v"), ("region", "eu")]);
    assert_eq!(props(b), [("k", "v"), ("region", "us")]);
}

#[test]
fn duplicate_names_resolve_last_wins_case_insensitively() {
    static CACHE: StaticSetCache<3> = StaticSetCache::new();
    let set = CACHE.get(
        PropertySet::empty(),
        [("k", "a"), ("other", "o"), ("K", "b")],
    );
    assert_eq!(props(set), [("K", "b"), ("other", "o")]);
}

#[test]
fn threads_racing_on_a_cold_key_get_the_same_set() {
    static CACHE: StaticSetCache<1> = StaticSetCache::new();
    let barrier = Barrier::new(8);
    let addresses: Vec<usize> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                s.spawn(|| {
                    barrier.wait();
                    CACHE.get(PropertySet::empty(), [("k", "race")]) as *const PropertySet as usize
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert!(addresses.windows(2).all(|w| w[0] == w[1]));
}
