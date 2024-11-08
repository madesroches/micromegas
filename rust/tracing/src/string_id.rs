//! StringId serializes the value of the pointer and the size
//! Also provides a facility to extract a StaticString from it
use micromegas_transit::{InProcSerialize, Member, Reflect, StaticString, UserDefinedType};

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

impl std::convert::From<&StringId> for StaticString {
    fn from(src: &StringId) -> Self {
        Self {
            len: src.len,
            ptr: src.ptr,
        }
    }
}

impl Reflect for StringId {
    fn reflect() -> UserDefinedType {
        UserDefinedType {
            name: String::from("StringId"),
            size: std::mem::size_of::<Self>(),
            members: vec![Member {
                name: "id".to_string(),
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

        let string_id = unsafe { StringId::read_value(buffer.as_ptr(), None) };
        assert_eq!(string_id.len, 5);
        assert_eq!(string_id.ptr, "hello".as_ptr());
    }
}
