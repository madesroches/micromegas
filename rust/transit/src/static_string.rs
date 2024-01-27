use crate::{read_any, write_any, InProcSerialize, InProcSize, Reflect, UserDefinedType};

// StaticString serializes the value of the pointer and the contents of the
// string
#[derive(Debug)]
pub struct StaticString {
    pub len: u32,
    pub ptr: *const u8,
}

impl std::convert::From<&str> for StaticString {
    fn from(src: &str) -> Self {
        Self {
            len: src.len() as u32,
            ptr: src.as_ptr(),
        }
    }
}

// dummy impl for Reflect
impl Reflect for StaticString {
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

impl InProcSerialize for StaticString {
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
    unsafe fn read_value(ptr: *const u8, value_size_opt: Option<u32>) -> Self {
        let id_size = std::mem::size_of::<usize>() as u32;
        let value_size = value_size_opt.unwrap();
        assert!(id_size <= value_size);
        let buffer_size = value_size - id_size;
        let static_buffer_ptr = read_any::<*const u8>(ptr);
        Self {
            len: buffer_size,
            ptr: static_buffer_ptr,
        }
    }
}
