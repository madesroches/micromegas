//! StaticStringRef points to a string dependency keeping track of the codec.
//! Necessary for unreal instrumentation where ansi and wide strings can coexist.
//! In cases where the event format does not have to be compatible with unreal, StringId can be used.
use micromegas_transit::{
    prelude::*, string_codec::StringCodec, Member, StaticStringDependency, UserDefinedType,
};

#[derive(Debug)]
pub struct StaticStringRef {
    pub ptr: *const u8,
    pub len: u32,
    pub codec: StringCodec,
}

impl InProcSerialize for StaticStringRef {}

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
        self.ptr as u64
    }

    pub fn into_dependency(&self) -> StaticStringDependency {
        StaticStringDependency {
            codec: self.codec,
            len: self.len,
            ptr: self.ptr,
        }
    }
}

impl std::convert::From<&'static str> for StaticStringRef {
    fn from(src: &'static str) -> Self {
        Self {
            len: src.len() as u32,
            ptr: src.as_ptr(),
            codec: StringCodec::Utf8,
        }
    }
}
