use micromegas_tracing::string_id::StringId;
use micromegas_transit::InProcSerialize;

#[test]
fn test_string_id() {
    let string_id = StringId::from("hello");
    assert_eq!(string_id.len, 5);
    assert_eq!(string_id.ptr, "hello".as_ptr());
    assert_eq!(string_id.id(), "hello".as_ptr() as u64);

    let mut buffer = vec![];
    string_id.write_value(&mut buffer);
    assert_eq!(buffer.len(), std::mem::size_of::<StringId>());

    let string_id = unsafe { StringId::read_value(&buffer) };
    assert_eq!(string_id.len, 5);
    assert_eq!(string_id.ptr, "hello".as_ptr());
}
