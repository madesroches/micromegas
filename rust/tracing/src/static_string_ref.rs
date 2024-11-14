//! StaticStringRef points to a string dependency keeping track of the codec.
//! Necessary for unreal instrumentation where ansi and wide strings can coexist.
//! In cases where the event format does not have to be compatible with unreal, StringId can be used.
use micromegas_transit::{
    prelude::*, string_codec::StringCodec, Member, StaticStringDependency, UserDefinedType,
};
use std::hash::{Hash, Hasher};

#[derive(Eq, PartialEq, Debug, Clone)]
pub struct StaticStringRef {
    pub ptr: u64,
    pub len: u32,
    pub codec: StringCodec,
}

impl Hash for StaticStringRef {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ptr.hash(state);
    }
}

impl Reflect for StaticStringRef {
    fn reflect() -> UserDefinedType {
        UserDefinedType {
            name: String::from("StaticStringRef"),
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

impl StaticStringRef {
    pub fn id(&self) -> u64 {
        self.ptr
    }

    pub fn into_dependency(&self) -> StaticStringDependency {
        StaticStringDependency {
            codec: self.codec,
            len: self.len,
            ptr: self.ptr as *const u8,
        }
    }
}

impl std::convert::From<&'static str> for StaticStringRef {
    fn from(src: &'static str) -> Self {
        Self {
            len: src.len() as u32,
            ptr: src.as_ptr() as u64,
            codec: StringCodec::Utf8,
        }
    }
}
