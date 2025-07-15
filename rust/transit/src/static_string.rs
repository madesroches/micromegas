use crate::{
    InProcSerialize, InProcSize, Reflect, UserDefinedType, read_consume_pod,
    string_codec::StringCodec, write_any,
};
use lazy_static::lazy_static;
use std::sync::Arc;

/// Utf8StaticStringDependency serializes the value of the pointer and the contents of the string
/// It should not be part of the event - it's the dependency of the StringId
#[derive(Debug)]
pub struct Utf8StaticStringDependency {
    pub len: u32,
    pub ptr: *const u8,
}

impl std::convert::From<&str> for Utf8StaticStringDependency {
    fn from(src: &str) -> Self {
        Self {
            len: src.len() as u32,
            ptr: src.as_ptr(),
        }
    }
}

// dummy impl for Reflect
impl Reflect for Utf8StaticStringDependency {
    fn reflect() -> UserDefinedType {
        lazy_static! {
            static ref TYPE_NAME: Arc<String> = Arc::new("StaticString".into());
        }
        UserDefinedType {
            name: TYPE_NAME.clone(),
            size: 0,
            members: vec![],
            is_reference: false,
            secondary_udts: vec![],
        }
    }
}

impl InProcSerialize for Utf8StaticStringDependency {
    const IN_PROC_SIZE: InProcSize = InProcSize::Dynamic;

    fn get_value_size(&self) -> Option<u32> {
        let id_size = std::mem::size_of::<usize>() as u32;
        Some(self.len + id_size)
    }

    #[allow(unsafe_code)]
    fn write_value(&self, buffer: &mut Vec<u8>) {
        write_any(buffer, &self.ptr);
        unsafe {
            let slice = std::slice::from_raw_parts(self.ptr, self.len as usize);
            buffer.extend_from_slice(slice);
        }
    }

    #[allow(unsafe_code)]
    unsafe fn read_value(mut window: &[u8]) -> Self {
        let static_buffer_ptr: *const u8 = read_consume_pod(&mut window);
        let buffer_size = window.len() as u32;
        Self {
            len: buffer_size,
            ptr: static_buffer_ptr,
        }
    }
}

/// StaticStringDependency serializes the value of the pointer and the contents of the string
/// It is designed to be wire-compatible with the unreal instrumentation
#[derive(Debug)]
pub struct StaticStringDependency {
    pub codec: StringCodec,
    pub len: u32,
    pub ptr: *const u8,
}

// dummy impl for Reflect
impl Reflect for StaticStringDependency {
    fn reflect() -> UserDefinedType {
        lazy_static! {
            static ref TYPE_NAME: Arc<String> = Arc::new("StaticStringDependency".into());
        }
        UserDefinedType {
            name: TYPE_NAME.clone(),
            size: 0,
            members: vec![],
            is_reference: false,
            secondary_udts: vec![],
        }
    }
}

impl InProcSerialize for StaticStringDependency {
    const IN_PROC_SIZE: InProcSize = InProcSize::Dynamic;

    fn get_value_size(&self) -> Option<u32> {
        let id_size = std::mem::size_of::<usize>() as u32;
        let size =
			id_size +
			1 + // codec
			std::mem::size_of::<u32>() as u32 +// size in bytes
			self.len // actual buffer
			;
        Some(size)
    }

    #[allow(unsafe_code)]
    fn write_value(&self, buffer: &mut Vec<u8>) {
        let id = self.ptr as u64;
        write_any(buffer, &id);
        let codec = self.codec as u8;
        write_any(buffer, &codec);
        write_any(buffer, &self.len);
        unsafe {
            let slice = std::slice::from_raw_parts(self.ptr, self.len as usize);
            buffer.extend_from_slice(slice);
        }
    }

    #[allow(unsafe_code)]
    unsafe fn read_value(_window: &[u8]) -> Self {
        // dependencies don't need to be read in the instrumented process
        unimplemented!();
    }
}
