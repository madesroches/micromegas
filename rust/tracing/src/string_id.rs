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

/// Names a string record in the dynamic dependency queue of the block that holds the event.
#[derive(Debug)]
pub struct DynStringId {
    pub id: u64,
}

// TransitReflect derive macro does not support reference udts, only reference members
impl Reflect for DynStringId {
    fn reflect() -> UserDefinedType {
        lazy_static::lazy_static! {
            static ref TYPE_NAME: Arc<String> = Arc::new("DynStringId".into());
            static ref ID: Arc<String> = Arc::new("id".into());
        }
        UserDefinedType {
            name: TYPE_NAME.clone(),
            size: std::mem::size_of::<Self>(),
            members: vec![Member {
                name: ID.clone(),
                type_name: "u64".to_string(),
                offset: memoffset::offset_of!(Self, id),
                size: std::mem::size_of::<u64>(),
                is_reference: true,
            }],
            is_reference: true,
            secondary_udts: vec![],
        }
    }
}

impl InProcSerialize for DynStringId {}
