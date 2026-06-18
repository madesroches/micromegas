use crate::{InProcSerialize, InProcSize, read_consume_pod, write_any};

#[derive(Debug)]
pub struct DynBlob(pub Vec<u8>);

impl InProcSerialize for DynBlob {
    const IN_PROC_SIZE: InProcSize = InProcSize::Dynamic;

    fn get_value_size(&self) -> Option<u32> {
        Some(std::mem::size_of::<u32>() as u32 + self.0.len() as u32)
    }

    fn write_value(&self, buffer: &mut Vec<u8>) {
        let len = self.0.len() as u32;
        write_any(buffer, &len);
        buffer.extend_from_slice(&self.0);
    }

    #[allow(unsafe_code)]
    unsafe fn read_value(mut window: &[u8]) -> Self {
        let len: u32 = read_consume_pod(&mut window);
        let data = window[..len as usize].to_vec();
        Self(data)
    }
}
