//! Regression tests hardening the transit parse path against malformed input.
//!
//! Covers the fallible helpers (`try_read_consume_pod`, `try_read_pod_at`,
//! `try_advance_window`), the string decoders (`read_advance_string`,
//! `read_advance_string_in`, including the UTF-16 `Wide` codec, which no Rust
//! write path emits but which interop clients can produce), and the
//! `read_dependencies` / `parse_object_buffer` / `parse_pod_instance` parse
//! loops with hand-built hostile buffers. Every case must return `Err`, never
//! panic.

use bumpalo::Bump;
use micromegas_transit::string_codec::StringCodec;
use micromegas_transit::value::{Object, Value};
use micromegas_transit::{
    CustomReaderMap, Member, UserDefinedType, parse_object_buffer, parse_pod_instance,
    read_advance_string, read_advance_string_in, read_dependencies, try_advance_window,
    try_read_consume_pod, try_read_pod_at,
};
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Fallible helpers (serialize.rs)
// ---------------------------------------------------------------------------

#[test]
fn try_read_consume_pod_rejects_empty_window() {
    let mut window: &[u8] = &[];
    let res: anyhow::Result<u32> = try_read_consume_pod(&mut window);
    assert!(res.is_err());
}

#[test]
fn try_read_consume_pod_rejects_short_window() {
    let mut window: &[u8] = &[1, 2, 3];
    let res: anyhow::Result<u64> = try_read_consume_pod(&mut window);
    assert!(res.is_err());
}

#[test]
fn try_read_consume_pod_succeeds_on_exact_window() {
    let bytes = 42u32.to_ne_bytes();
    let mut window: &[u8] = &bytes;
    let value: u32 = try_read_consume_pod(&mut window).unwrap();
    assert_eq!(value, 42);
    assert!(window.is_empty());
}

#[test]
fn try_read_pod_at_rejects_out_of_bounds_offset() {
    let buffer = [0u8; 4];
    let res: anyhow::Result<u64> = try_read_pod_at(&buffer, 0);
    assert!(res.is_err());
}

#[test]
fn try_read_pod_at_rejects_offset_overflow() {
    let buffer = [0u8; 4];
    let res: anyhow::Result<u8> = try_read_pod_at(&buffer, usize::MAX);
    assert!(res.is_err());
}

#[test]
fn try_read_pod_at_succeeds_in_bounds() {
    let buffer = 7u32.to_ne_bytes();
    let value: u32 = try_read_pod_at(&buffer, 0).unwrap();
    assert_eq!(value, 7);
}

#[test]
fn try_advance_window_rejects_overrun() {
    let buffer = [0u8; 4];
    assert!(try_advance_window(&buffer, 5).is_err());
}

#[test]
fn try_advance_window_allows_full_consume() {
    let buffer = [0u8; 4];
    let rest = try_advance_window(&buffer, 4).unwrap();
    assert!(rest.is_empty());
}

// ---------------------------------------------------------------------------
// String decoders (dyn_string.rs)
// ---------------------------------------------------------------------------

fn wide_string_buffer(units: &[u16]) -> Vec<u8> {
    let mut buf = vec![StringCodec::Wide as u8];
    let byte_len = (units.len() * 2) as u32;
    buf.extend_from_slice(&byte_len.to_le_bytes());
    for u in units {
        buf.extend_from_slice(&u.to_le_bytes());
    }
    buf
}

#[test]
fn read_advance_string_rejects_empty_window() {
    let mut window: &[u8] = &[];
    assert!(read_advance_string(&mut window).is_err());
}

#[test]
fn read_advance_string_rejects_invalid_codec() {
    let buf = vec![9u8, 0, 0, 0, 0];
    let mut window: &[u8] = &buf;
    assert!(read_advance_string(&mut window).is_err());
}

#[test]
fn read_advance_string_rejects_declared_length_exceeding_window() {
    let mut buf = vec![StringCodec::Utf8 as u8];
    buf.extend_from_slice(&100u32.to_le_bytes());
    buf.extend_from_slice(b"short");
    let mut window: &[u8] = &buf;
    assert!(read_advance_string(&mut window).is_err());
}

#[test]
fn read_advance_string_rejects_odd_wide_length() {
    let mut buf = vec![StringCodec::Wide as u8];
    buf.extend_from_slice(&3u32.to_le_bytes());
    buf.extend_from_slice(&[0, 0, 0]);
    let mut window: &[u8] = &buf;
    assert!(read_advance_string(&mut window).is_err());
}

#[test]
fn read_advance_string_rejects_truncated_wide() {
    let mut buf = vec![StringCodec::Wide as u8];
    buf.extend_from_slice(&4u32.to_le_bytes()); // declares 2 utf-16 units
    buf.extend_from_slice(&[0, 0]); // but only 1 is present
    let mut window: &[u8] = &buf;
    assert!(read_advance_string(&mut window).is_err());
}

#[test]
fn read_advance_string_decodes_valid_wide() {
    let units: Vec<u16> = "hi".encode_utf16().collect();
    let buf = wide_string_buffer(&units);
    let mut window: &[u8] = &buf;
    let s = read_advance_string(&mut window).unwrap();
    assert_eq!(s, "hi");
    assert!(window.is_empty());
}

#[test]
fn read_advance_string_in_rejects_empty_window() {
    let bump = Bump::new();
    let mut window: &[u8] = &[];
    assert!(read_advance_string_in(&bump, &mut window).is_err());
}

#[test]
fn read_advance_string_in_rejects_declared_length_exceeding_window() {
    let bump = Bump::new();
    let mut buf = vec![StringCodec::Utf8 as u8];
    buf.extend_from_slice(&100u32.to_le_bytes());
    buf.extend_from_slice(b"short");
    let mut window: &[u8] = &buf;
    assert!(read_advance_string_in(&bump, &mut window).is_err());
}

#[test]
fn read_advance_string_in_rejects_odd_wide_length() {
    let bump = Bump::new();
    let mut buf = vec![StringCodec::Wide as u8];
    buf.extend_from_slice(&3u32.to_le_bytes());
    buf.extend_from_slice(&[0, 0, 0]);
    let mut window: &[u8] = &buf;
    assert!(read_advance_string_in(&bump, &mut window).is_err());
}

#[test]
fn read_advance_string_in_rejects_truncated_wide() {
    let bump = Bump::new();
    let mut buf = vec![StringCodec::Wide as u8];
    buf.extend_from_slice(&4u32.to_le_bytes());
    buf.extend_from_slice(&[0, 0]);
    let mut window: &[u8] = &buf;
    assert!(read_advance_string_in(&bump, &mut window).is_err());
}

#[test]
fn read_advance_string_in_decodes_valid_wide() {
    let bump = Bump::new();
    let units: Vec<u16> = "hi".encode_utf16().collect();
    let buf = wide_string_buffer(&units);
    let mut window: &[u8] = &buf;
    let s = read_advance_string_in(&bump, &mut window).unwrap();
    assert_eq!(s, "hi");
    assert!(window.is_empty());
}

// ---------------------------------------------------------------------------
// read_dependencies / parse_object_buffer / parse_pod_instance (parser.rs)
// ---------------------------------------------------------------------------

fn udt_named(name: &str, size: usize) -> UserDefinedType {
    UserDefinedType {
        name: Arc::new(name.to_string()),
        size,
        members: vec![],
        is_reference: false,
        secondary_udts: vec![],
    }
}

fn pod_udt(name: &str, size: usize, members: Vec<Member>) -> UserDefinedType {
    UserDefinedType {
        name: Arc::new(name.to_string()),
        size,
        members,
        is_reference: false,
        secondary_udts: vec![],
    }
}

fn member(name: &str, type_name: &str, offset: usize, size: usize, is_reference: bool) -> Member {
    Member {
        name: Arc::new(name.to_string()),
        type_name: type_name.to_string(),
        offset,
        size,
        is_reference,
    }
}

fn static_string_entry(type_index: u8, string_id: u64, s: &str) -> Vec<u8> {
    let mut buf = vec![type_index];
    let object_size = std::mem::size_of::<u64>() + s.len();
    buf.extend_from_slice(&(object_size as u32).to_le_bytes());
    buf.extend_from_slice(&string_id.to_le_bytes());
    buf.extend_from_slice(s.as_bytes());
    buf
}

fn static_string_dependency_entry(type_index: u8, string_id: u64, s: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&string_id.to_le_bytes());
    payload.push(StringCodec::Utf8 as u8);
    payload.extend_from_slice(&(s.len() as u32).to_le_bytes());
    payload.extend_from_slice(s.as_bytes());
    let mut buf = vec![type_index];
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(&payload);
    buf
}

fn test_custom_reader<'a>(
    bump: &'a Bump,
    _udt: &'a UserDefinedType,
    _udts: &'a [UserDefinedType],
    _deps: &HashMap<u64, Value<'a>>,
    window: &'a [u8],
) -> anyhow::Result<Value<'a>> {
    let id = try_read_pod_at::<u64>(window, 0)?;
    let members = bump.alloc_slice_copy(&[("id", Value::U64(id)), ("value", Value::U64(id))]);
    Ok(Value::Object(bump.alloc(Object {
        type_name: "CustomDep",
        members,
    })))
}

#[test]
fn read_dependencies_rejects_invalid_type_index() {
    let bump = Bump::new();
    let udts: Vec<UserDefinedType> = vec![];
    let buffer = vec![0u8];
    let custom_readers = CustomReaderMap::new();
    assert!(read_dependencies(&bump, &custom_readers, &udts, &buffer).is_err());
}

#[test]
fn read_dependencies_rejects_truncated_dynamic_size() {
    let bump = Bump::new();
    let udts = vec![udt_named("StaticString", 0)];
    let buffer = vec![0u8]; // type index only, missing the u32 size
    let custom_readers = CustomReaderMap::new();
    assert!(read_dependencies(&bump, &custom_readers, &udts, &buffer).is_err());
}

#[test]
fn read_dependencies_rejects_object_exceeding_buffer() {
    let bump = Bump::new();
    let udts = vec![udt_named("StaticString", 0)];
    let mut buffer = vec![0u8];
    buffer.extend_from_slice(&100u32.to_le_bytes()); // declared size far exceeds buffer
    let custom_readers = CustomReaderMap::new();
    assert!(read_dependencies(&bump, &custom_readers, &udts, &buffer).is_err());
}

#[test]
fn read_dependencies_rejects_static_string_smaller_than_header() {
    let bump = Bump::new();
    let udts = vec![udt_named("StaticString", 0)];
    let mut buffer = vec![0u8];
    buffer.extend_from_slice(&4u32.to_le_bytes()); // object_size 4 < size_of::<usize>()
    buffer.extend_from_slice(&[0u8; 4]);
    let custom_readers = CustomReaderMap::new();
    assert!(read_dependencies(&bump, &custom_readers, &udts, &buffer).is_err());
}

#[test]
fn read_dependencies_rejects_duplicate_id_static_string_branch() {
    let bump = Bump::new();
    let udts = vec![udt_named("StaticString", 0)];
    let mut buffer = static_string_entry(0, 1, "ab");
    buffer.extend(static_string_entry(0, 1, "cd"));
    let custom_readers = CustomReaderMap::new();
    assert!(read_dependencies(&bump, &custom_readers, &udts, &buffer).is_err());
}

#[test]
fn read_dependencies_accepts_unique_static_strings() {
    let bump = Bump::new();
    let udts = vec![udt_named("StaticString", 0)];
    let mut buffer = static_string_entry(0, 1, "ab");
    buffer.extend(static_string_entry(0, 2, "cd"));
    let custom_readers = CustomReaderMap::new();
    let deps = read_dependencies(&bump, &custom_readers, &udts, &buffer).unwrap();
    assert_eq!(deps.len(), 2);
}

#[test]
fn read_dependencies_rejects_duplicate_id_static_string_dependency_branch() {
    let bump = Bump::new();
    let udts = vec![udt_named("StaticStringDependency", 0)];
    let mut buffer = static_string_dependency_entry(0, 3, "ab");
    buffer.extend(static_string_dependency_entry(0, 3, "cd"));
    let custom_readers = CustomReaderMap::new();
    assert!(read_dependencies(&bump, &custom_readers, &udts, &buffer).is_err());
}

#[test]
fn read_dependencies_rejects_truncated_static_string_dependency() {
    let bump = Bump::new();
    let udts = vec![udt_named("StaticStringDependency", 0)];
    // A declared object_size of 0 trivially passes the window-fits guard, but
    // the StaticStringDependency branch hands the reader the *entire*
    // remaining buffer (not bounded to object_size) — with no bytes left,
    // the string_id read must fail via try_read_consume_pod.
    let mut buffer = vec![0u8];
    buffer.extend_from_slice(&0u32.to_le_bytes());
    let custom_readers = CustomReaderMap::new();
    assert!(read_dependencies(&bump, &custom_readers, &udts, &buffer).is_err());
}

#[test]
fn read_dependencies_rejects_duplicate_id_pod_branch() {
    let bump = Bump::new();
    let udt = pod_udt("MyDep", 8, vec![member("id", "u64", 0, 8, false)]);
    let udts = vec![udt];
    let mut buffer = vec![0u8];
    buffer.extend_from_slice(&5u64.to_le_bytes());
    buffer.push(0u8);
    buffer.extend_from_slice(&5u64.to_le_bytes()); // duplicate id
    let custom_readers = CustomReaderMap::new();
    assert!(read_dependencies(&bump, &custom_readers, &udts, &buffer).is_err());
}

#[test]
fn read_dependencies_rejects_duplicate_id_custom_reader_branch() {
    let bump = Bump::new();
    let udts = vec![pod_udt("CustomDep", 0, vec![])];
    let mut buffer = vec![0u8];
    buffer.extend_from_slice(&8u32.to_le_bytes());
    buffer.extend_from_slice(&9u64.to_le_bytes());
    buffer.push(0u8);
    buffer.extend_from_slice(&8u32.to_le_bytes());
    buffer.extend_from_slice(&9u64.to_le_bytes()); // duplicate id
    let mut custom_readers: CustomReaderMap = HashMap::new();
    custom_readers.insert("CustomDep".to_string(), Arc::new(test_custom_reader));
    assert!(read_dependencies(&bump, &custom_readers, &udts, &buffer).is_err());
}

#[test]
fn parse_object_buffer_rejects_truncated_dynamic_size() {
    let bump = Bump::new();
    let udts = vec![pod_udt("Custom", 0, vec![])];
    let buffer = vec![0u8];
    let deps = HashMap::new();
    let custom_readers = CustomReaderMap::new();
    let res = parse_object_buffer(&bump, &custom_readers, &deps, &udts, &buffer, |_| Ok(true));
    assert!(res.is_err());
}

#[test]
fn parse_object_buffer_rejects_static_object_exceeding_buffer() {
    let bump = Bump::new();
    let udts = vec![pod_udt("U32Val", 4, vec![member("x", "u32", 0, 4, false)])];
    let buffer = vec![0u8, 1, 2]; // only 2 bytes follow, need 4
    let deps = HashMap::new();
    let custom_readers = CustomReaderMap::new();
    let res = parse_object_buffer(&bump, &custom_readers, &deps, &udts, &buffer, |_| Ok(true));
    assert!(res.is_err());
}

#[test]
fn parse_object_buffer_parses_valid_pod() {
    let bump = Bump::new();
    let udts = vec![pod_udt("U32Val", 4, vec![member("x", "u32", 0, 4, false)])];
    let mut buffer = vec![0u8];
    buffer.extend_from_slice(&7u32.to_le_bytes());
    let deps = HashMap::new();
    let custom_readers = CustomReaderMap::new();
    let mut seen = 0;
    parse_object_buffer(&bump, &custom_readers, &deps, &udts, &buffer, |v| {
        if let Value::Object(obj) = v {
            assert_eq!(obj.get::<u32>("x").unwrap(), 7);
        } else {
            panic!("expected object");
        }
        seen += 1;
        Ok(true)
    })
    .unwrap();
    assert_eq!(seen, 1);
}

#[test]
fn parse_pod_instance_rejects_member_exceeding_window() {
    let bump = Bump::new();
    let udt = pod_udt("U32Val", 4, vec![member("x", "u32", 0, 4, false)]);
    let udts = vec![udt];
    let deps = HashMap::new();
    let window: [u8; 2] = [0, 0];
    assert!(parse_pod_instance(&bump, &udts[0], &udts, &deps, &window).is_err());
}

#[test]
fn parse_pod_instance_rejects_type_size_mismatch() {
    let bump = Bump::new();
    let udt = pod_udt("Bad", 8, vec![member("x", "u32", 0, 8, false)]);
    let udts = vec![udt];
    let deps = HashMap::new();
    let window = [0u8; 8];
    assert!(parse_pod_instance(&bump, &udts[0], &udts, &deps, &window).is_err());
}

#[test]
fn parse_pod_instance_rejects_short_reference_member() {
    let bump = Bump::new();
    let udt = pod_udt("Ref", 4, vec![member("id", "u64", 0, 4, true)]);
    let udts = vec![udt];
    let deps = HashMap::new();
    let window = [0u8; 4];
    assert!(parse_pod_instance(&bump, &udts[0], &udts, &deps, &window).is_err());
}

#[test]
fn parse_pod_instance_rejects_nested_udt_exceeding_window() {
    let bump = Bump::new();
    let inner = pod_udt("Inner", 8, vec![member("v", "u64", 0, 8, false)]);
    // member metadata declares a smaller size than the actual nested udt, so
    // the top-of-loop guard passes but the nested-udt guard must still catch it.
    let outer_member = member("inner", "Inner", 0, 4, false);
    let outer = pod_udt("Outer", 4, vec![outer_member]);
    let udts = vec![outer, inner];
    let deps = HashMap::new();
    let window = [0u8; 4];
    assert!(parse_pod_instance(&bump, &udts[0], &udts, &deps, &window).is_err());
}

#[test]
fn parse_pod_instance_rejects_unknown_member_type() {
    let bump = Bump::new();
    let udt = pod_udt("Weird", 4, vec![member("x", "does_not_exist", 0, 4, false)]);
    let udts = vec![udt];
    let deps = HashMap::new();
    let window = [0u8; 4];
    assert!(parse_pod_instance(&bump, &udts[0], &udts, &deps, &window).is_err());
}

#[test]
fn parse_pod_instance_rejects_missing_reference_dependency() {
    let bump = Bump::new();
    let udt = pod_udt("Ref", 8, vec![member("id", "u64", 0, 8, true)]);
    let udts = vec![udt];
    let deps: HashMap<u64, Value> = HashMap::new();
    let window = 123u64.to_ne_bytes();
    assert!(parse_pod_instance(&bump, &udts[0], &udts, &deps, &window).is_err());
}
