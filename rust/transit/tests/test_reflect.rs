use lgn_tracing_transit::prelude::*;

#[derive(TransitReflect)]
#[allow(dead_code)]
struct MyTestEvent {
    some_64: u64,
    some_32: u32,
}

#[test]
fn test_reflect_simple_struct() {
    let res = MyTestEvent::reflect();
    assert_eq!("MyTestEvent", res.name);
    assert_eq!(16, res.size);
    assert_eq!(2, res.members.len());
    assert_eq!("some_64", res.members[0].name);
    assert_eq!(8, res.members[0].size);
    assert_eq!(0, res.members[0].offset);
    assert_eq!("some_32", res.members[1].name);
    assert_eq!(4, res.members[1].size);
    assert_eq!(8, res.members[1].offset);
}

#[derive(TransitReflect)]
struct ScopeDesc {
    pub name: &'static str,
    pub filename: &'static str,
    pub line: u32,
}

#[derive(TransitReflect)]
struct BeginScopeEvent {
    pub time: u64,
    pub get_scope_desc: fn() -> ScopeDesc,
}

#[test]
fn test_reflect_scope_event() {
    let scope_desc_reflection = ScopeDesc::reflect();
    assert!(scope_desc_reflection.members[0].is_reference);
    assert!(scope_desc_reflection.members[1].is_reference);
    assert!(!scope_desc_reflection.members[2].is_reference);
    let event_reflection = BeginScopeEvent::reflect();
    assert!(!event_reflection.members[0].is_reference);
    assert!(event_reflection.members[1].is_reference);
}
