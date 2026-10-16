//! StaticStringRef points to a string dependency keeping track of the codec.
//! Necessary for unreal instrumentation where ansi and wide strings can coexist.
//! In cases where the event format does not have to be compatible with unreal, StringId can be used.
use micromegas_transit::{
    Member, StaticStringDependency, UserDefinedType, prelude::*, string_codec::StringCodec,
};
use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

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
        lazy_static::lazy_static! {
            static ref TYPE_NAME: Arc<String> = Arc::new("StaticStringRef".into());
            static ref ID: Arc<String> = Arc::new("id".into());
        }
        UserDefinedType {
            name: TYPE_NAME.clone(),
            size: std::mem::size_of::<Self>(),
            members: vec![Member {
                name: ID.clone(),
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
            id: self.ptr,
            codec: self.codec,
            len: self.len,
            ptr: self.ptr as *const u8,
        }
    }

    /// Builds a ref to `src` without requiring `'static`.
    ///
    /// # Safety
    /// `src` must outlive every use of the returned ref.
    pub(crate) unsafe fn from_str_unchecked(src: &str) -> Self {
        Self {
            len: src.len() as u32,
            ptr: src.as_ptr() as u64,
            codec: StringCodec::Utf8,
        }
    }

    pub fn as_str(&self) -> &str {
        unsafe {
            let slice = std::slice::from_raw_parts(self.ptr as *const u8, self.len as usize);
            std::str::from_utf8_unchecked(slice)
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

/// Reference to a string record in the block's dynamic dependency queue. It has the layout and
/// the reflection of `StaticStringRef`, so the wire format is the same, but it can't be
/// dereferenced: `id` is only meaningful to the `DynDeps` of its block.
#[derive(Eq, PartialEq, Debug, Clone, Copy)]
pub struct BlockStringRef {
    pub id: u64,
    pub len: u32,
    pub codec: StringCodec,
}

const _: () = {
    use std::mem::{offset_of, size_of};
    assert!(size_of::<BlockStringRef>() == size_of::<StaticStringRef>());
    assert!(offset_of!(BlockStringRef, id) == offset_of!(StaticStringRef, ptr));
    assert!(offset_of!(BlockStringRef, len) == offset_of!(StaticStringRef, len));
    assert!(offset_of!(BlockStringRef, codec) == offset_of!(StaticStringRef, codec));
};

impl Reflect for BlockStringRef {
    fn reflect() -> UserDefinedType {
        StaticStringRef::reflect()
    }
}
