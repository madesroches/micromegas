use anyhow::{bail, Result};

use crate::read_any;

#[allow(unsafe_code, clippy::cast_ptr_alignment)]
pub fn parse_string(buffer: &[u8], cursor: &mut usize) -> Result<String> {
    unsafe {
        let codec_id = buffer[*cursor];
        *cursor += 1;
        let string_len_bytes = read_any::<u32>(buffer.as_ptr().add(*cursor)) as usize;
        *cursor += std::mem::size_of::<u32>();
        let string_buffer = &buffer[(*cursor)..(*cursor + string_len_bytes)];
        *cursor += string_len_bytes;
        const ANSI_CODE: u8 = 0;
        const WIDE_CODE: u8 = 1;
        const UTF8_CODE: u8 = 2;
        match codec_id {
            ANSI_CODE => {
                // this would be typically be windows 1252, an extension to ISO-8859-1/latin1
                // random people on the interwebs tell me that latin1's codepoints are a subset of utf8
                // so I guess it's ok to treat it as utf8
                Ok(String::from_utf8_lossy(string_buffer).to_string())
            }
            WIDE_CODE => {
                //wide
                let ptr = string_buffer.as_ptr().cast::<u16>();
                if string_len_bytes % 2 != 0 {
                    anyhow::bail!("wrong utf-16 buffer size");
                }
                let wide_slice = std::ptr::slice_from_raw_parts(ptr, string_len_bytes as usize / 2);
                Ok(String::from_utf16_lossy(&*wide_slice))
            }
            UTF8_CODE => Ok(String::from_utf8_lossy(string_buffer).to_string()),
            other => {
                bail!("invalid codec [{}] in string", other);
            }
        }
    }
}
