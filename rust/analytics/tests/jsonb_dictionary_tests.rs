use datafusion::arrow::array::AsArray;
use micromegas_analytics::properties::jsonb_dictionary::JsonbDictionary;

#[test]
fn index_zero_is_the_empty_set_and_equal_bytes_share_an_index() {
    let mut dict = JsonbDictionary::new();
    let empty = dict.values()[0].clone();
    assert_eq!(dict.intern(&empty), 0);
    let a = dict.intern(b"a");
    let b = dict.intern(b"b");
    assert_ne!(a, b);
    assert_eq!(dict.intern(b"a"), a);
    assert_eq!(dict.values().len(), 3);
}

#[test]
fn finish_round_trips_the_keys() {
    let mut dict = JsonbDictionary::new();
    let a = dict.intern(b"a");
    let array = dict.finish(vec![a, 0, a]).unwrap();
    let values = array.values().as_binary::<i32>();
    assert_eq!(array.keys().len(), 3);
    assert_eq!(values.value(array.keys().value(0) as usize), b"a");
    assert_eq!(array.keys().value(1), 0);
    assert_eq!(array.keys().value(2), a);
}
