use crate::{
    read_consume_pod, string_codec::StringCodec, write_any, InProcSerialize, InProcSize, Reflect,
    UserDefinedType,
};

/// Utf8StaticString serializes the value of the pointer and the contents of the string
/// It should not be part of the event - it's the dependency of the StringId
#[derive(Debug)]
pub struct Utf8StaticString {
    pub len: u32,
    pub ptr: *const u8,
}

impl std::convert::From<&str> for Utf8StaticString {
    fn from(src: &str) -> Self {
        Self {
            len: src.len() as u32,
            ptr: src.as_ptr(),
        }
    }
}

// dummy impl for Reflect
impl Reflect for Utf8StaticString {
    fn reflect() -> UserDefinedType {
        UserDefinedType {
            name: String::from("StaticString"),
            size: 0,
            members: vec![],
            is_reference: false,
            secondary_udts: vec![],
        }
    }
}

impl InProcSerialize for Utf8StaticString {
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
        UserDefinedType {
            name: String::from("StaticStringDependency"),
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
    unsafe fn read_value(mut window: &[u8]) -> Self {
        let id: u64 = read_consume_pod(&mut window);
        let static_buffer_ptr: *const u8 = id as *const u8;
        let codec = StringCodec::try_from(read_consume_pod::<u8>(&mut window)).unwrap();
        let buffer_size: u32 = read_consume_pod(&mut window);
        assert_eq!(buffer_size as usize, window.len());
        Self {
            codec,
            len: buffer_size,
            ptr: static_buffer_ptr,
        }
    }
}
