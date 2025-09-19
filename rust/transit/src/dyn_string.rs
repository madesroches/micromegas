use crate::{
    InProcSerialize, InProcSize, advance_window, read_consume_pod, string_codec::StringCodec,
    write_any,
};
use anyhow::Result;

#[derive(Debug)]
pub struct LegacyDynString(pub String);

impl InProcSerialize for LegacyDynString {
    const IN_PROC_SIZE: InProcSize = InProcSize::Dynamic;

    fn get_value_size(&self) -> Option<u32> {
        Some(self.0.len() as u32)
    }

    fn write_value(&self, buffer: &mut Vec<u8>) {
        buffer.extend_from_slice(self.0.as_bytes());
    }

    #[allow(unsafe_code)]
    unsafe fn read_value(window: &[u8]) -> Self {
        Self(String::from_utf8(window.to_vec()).unwrap())
    }
}

#[derive(Debug)]
pub struct DynString(pub String);

impl InProcSerialize for DynString {
    const IN_PROC_SIZE: InProcSize = InProcSize::Dynamic;

    fn get_value_size(&self) -> Option<u32> {
        let header_size = 1 + // codec
			std::mem::size_of::<u32>() as u32 // size in bytes
			;
        let string_size = self.0.len() as u32;
        Some(header_size + string_size)
    }

    fn write_value(&self, buffer: &mut Vec<u8>) {
        let codec = StringCodec::Utf8 as u8;
        write_any(buffer, &codec);
        let len = self.0.len() as u32;
        write_any(buffer, &len);
        buffer.extend_from_slice(self.0.as_bytes());
    }

    #[allow(unsafe_code)]
    unsafe fn read_value(mut window: &[u8]) -> Self {
        let res = read_advance_string(&mut window).unwrap();
        assert_eq!(window.len(), 0);
        Self(res)
    }
}

/// Parse string from buffer, move buffer pointer forward.
#[allow(unsafe_code, clippy::cast_ptr_alignment)]
pub fn read_advance_string(window: &mut &[u8]) -> Result<String> {
    unsafe {
        let codec = StringCodec::try_from(read_consume_pod::<u8>(window))?;
        let string_len_bytes: u32 = read_consume_pod(window);
        let string_buffer = &window[0..(string_len_bytes as usize)];
        *window = advance_window(window, string_len_bytes as usize);
        match codec {
            StringCodec::Ansi => {
                // this would be typically be windows 1252, an extension to ISO-8859-1/latin1
                // random people on the interwebs tell me that latin1's codepoints are a subset of utf8
                // so I guess it's ok to treat it as utf8
                Ok(String::from_utf8_lossy(string_buffer).to_string())
            }
            StringCodec::Wide => {
                //wide
                let ptr = string_buffer.as_ptr().cast::<u16>();
                if !string_len_bytes.is_multiple_of(2) {
                    anyhow::bail!("wrong utf-16 buffer size");
                }
                let wide_slice = std::ptr::slice_from_raw_parts(ptr, string_len_bytes as usize / 2);
                Ok(String::from_utf16_lossy(&*wide_slice))
            }
            StringCodec::Utf8 => Ok(String::from_utf8_lossy(string_buffer).to_string()),
        }
    }
}
