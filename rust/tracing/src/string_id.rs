//! StringId serializes the value of the pointer and the size of a UTF8 string.
//! StaticStringRef should be prefered where wire compatibility with unreal is important.
use std::sync::Arc;

use micromegas_transit::{
    InProcSerialize, Member, Reflect, UserDefinedType, Utf8StaticStringDependency,
};

#[derive(Debug)]
pub struct StringId {
    pub ptr: *const u8,
    pub len: u32,
}

impl std::convert::From<&'static str> for StringId {
    fn from(src: &'static str) -> Self {
        Self {
            len: src.len() as u32,
            ptr: src.as_ptr(),
        }
    }
}

impl std::convert::From<&StringId> for Utf8StaticStringDependency {
    fn from(src: &StringId) -> Self {
        Self {
            len: src.len,
            ptr: src.ptr,
        }
    }
}

// TransitReflect derive macro does not support reference udts, only reference members
impl Reflect for StringId {
    fn reflect() -> UserDefinedType {
        lazy_static::lazy_static! {
            static ref TYPE_NAME: Arc<String> = Arc::new("StringId".into());
            static ref ID: Arc<String> = Arc::new("id".into());
        }
        UserDefinedType {
            name: TYPE_NAME.clone(),
            size: std::mem::size_of::<Self>(),
            members: vec![Member {
                name: ID.clone(), // reference udt need a member named id that's 8 bytes
                type_name: "usize".to_string(),
                offset: memoffset::offset_of!(Self, ptr),
                size: std::mem::size_of::<*const u8>(),
                is_reference: true,
            }],
            is_reference: true,
            secondary_udts: vec![],
        }
    }
}

impl InProcSerialize for StringId {}

impl StringId {
    pub fn id(&self) -> u64 {
        self.ptr as u64
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn test_string_id() {
        use super::*;
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
}
