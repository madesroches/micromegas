use crate::{InProcSerialize, InProcSize};

#[derive(Debug)]
pub struct DynString(pub String);

impl InProcSerialize for DynString {
    const IN_PROC_SIZE: InProcSize = InProcSize::Dynamic;

    fn get_value_size(&self) -> Option<u32> {
        Some(self.0.len() as u32)
    }

    fn write_value(&self, buffer: &mut Vec<u8>) {
        buffer.extend_from_slice(self.0.as_bytes());
    }

    #[allow(unsafe_code)]
    unsafe fn read_value(ptr: *const u8, value_size: Option<u32>) -> Self {
        let buffer_size = value_size.unwrap();
        let slice = std::ptr::slice_from_raw_parts(ptr, buffer_size as usize);
        Self(String::from_utf8((*slice).to_vec()).unwrap())
    }
}
