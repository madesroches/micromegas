use micromegas_capi::{metric_properties, read_properties};
use micromegas_tracing::dyn_deps::merge_properties;
use micromegas_tracing::property_set::PropertySet;
use std::ffi::CString;
use std::os::raw::c_char;
use std::ptr;

fn pairs<'a>(
    properties: &[micromegas_tracing::property_set::DynProperty<'a>],
) -> Vec<(&'a str, &'a str)> {
    properties.iter().map(|p| (p.name(), p.value())).collect()
}

#[test]
fn null_empty_and_non_utf8_keys_and_null_values_are_dropped() {
    let good = CString::new("good").unwrap();
    let empty = CString::new("").unwrap();
    let bad_utf8 = CString::new(vec![0xff, 0xfe]).unwrap();
    let value = CString::new("v").unwrap();
    let keys: [*const c_char; 5] = [
        good.as_ptr(),
        ptr::null(),
        empty.as_ptr(),
        bad_utf8.as_ptr(),
        good.as_ptr(),
    ];
    let values: [*const c_char; 5] = [
        value.as_ptr(),
        value.as_ptr(),
        value.as_ptr(),
        value.as_ptr(),
        ptr::null(),
    ];
    let properties = unsafe { read_properties(keys.as_ptr(), values.as_ptr(), 5) };
    assert_eq!(pairs(&properties), vec![("good", "v")]);
}

#[test]
fn non_utf8_value_is_kept_as_empty() {
    let key = CString::new("k").unwrap();
    let bad_utf8 = CString::new(vec![0xff, 0xfe]).unwrap();
    let keys = [key.as_ptr()];
    let values = [bad_utf8.as_ptr()];
    let properties = unsafe { read_properties(keys.as_ptr(), values.as_ptr(), 1) };
    assert_eq!(pairs(&properties), vec![("k", "")]);
}

#[test]
fn null_arrays_with_a_count_yield_no_pairs() {
    let key = CString::new("k").unwrap();
    let keys = [key.as_ptr()];
    assert!(unsafe { read_properties(ptr::null(), ptr::null(), 3) }.is_empty());
    assert!(unsafe { read_properties(keys.as_ptr(), ptr::null(), 1) }.is_empty());
    assert!(unsafe { read_properties(ptr::null(), keys.as_ptr(), 1) }.is_empty());
    assert!(unsafe { read_properties(keys.as_ptr(), keys.as_ptr(), 0) }.is_empty());
}

#[test]
fn metric_properties_end_with_name_and_unit() {
    let region = CString::new("region").unwrap();
    let eu = CString::new("eu").unwrap();
    let keys = [region.as_ptr()];
    let values = [eu.as_ptr()];
    let properties =
        unsafe { metric_properties(keys.as_ptr(), values.as_ptr(), 1, "queue.depth", "count") };
    assert_eq!(
        pairs(&properties),
        vec![("region", "eu"), ("name", "queue.depth"), ("unit", "count")]
    );
}

#[test]
fn metric_arguments_win_over_keys_of_the_same_name_and_target_is_kept() {
    let name_key = CString::new("Name").unwrap();
    let unit_key = CString::new("unit").unwrap();
    let target_key = CString::new("target").unwrap();
    let other = CString::new("other").unwrap();
    let keys = [name_key.as_ptr(), unit_key.as_ptr(), target_key.as_ptr()];
    let values = [other.as_ptr(), other.as_ptr(), other.as_ptr()];
    let properties =
        unsafe { metric_properties(keys.as_ptr(), values.as_ptr(), 3, "queue.depth", "count") };
    let merged = merge_properties(PropertySet::empty(), &properties);
    let mut merged_pairs = pairs(merged.properties());
    merged_pairs.sort();
    assert_eq!(
        merged_pairs,
        vec![
            ("name", "queue.depth"),
            ("target", "other"),
            ("unit", "count")
        ]
    );
}
