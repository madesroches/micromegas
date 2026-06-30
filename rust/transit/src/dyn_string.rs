use crate::{
    InProcSerialize, InProcSize, advance_window, read_consume_pod, string_codec::StringCodec,
    write_any,
};
use anyhow::Result;
use bumpalo::Bump;
use std::borrow::Cow;

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

/// Parse a string from the buffer, moving the buffer pointer forward.
///
/// Borrows the source buffer (`'a`) where possible: a valid-UTF-8 string is
/// returned as a zero-copy slice of the buffer. Only the transcoded cases
/// (UTF-16 wide, or lossy replacement of invalid bytes) allocate into the arena.
pub fn read_advance_string_in<'a>(bump: &'a Bump, window: &mut &'a [u8]) -> Result<&'a str> {
    let codec = StringCodec::try_from(read_consume_pod::<u8>(window))?;
    let string_len_bytes: u32 = read_consume_pod(window);
    let string_buffer = &window[0..(string_len_bytes as usize)];
    *window = advance_window(window, string_len_bytes as usize);
    match codec {
        // Treat ANSI (windows-1252/latin1) as utf8, matching read_advance_string.
        StringCodec::Ansi | StringCodec::Utf8 => match String::from_utf8_lossy(string_buffer) {
            // Valid UTF-8: borrow the source buffer directly (zero-copy).
            Cow::Borrowed(s) => Ok(s),
            // Invalid bytes were replaced: the transcoded result lives in the arena.
            Cow::Owned(s) => Ok(bump.alloc_str(&s)),
        },
        StringCodec::Wide => {
            if !string_len_bytes.is_multiple_of(2) {
                anyhow::bail!("wrong utf-16 buffer size");
            }
            // Decode UTF-16 LE without assuming the source bytes are 2-byte aligned.
            let units = string_buffer
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
            let s: String = char::decode_utf16(units)
                .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect();
            Ok(bump.alloc_str(&s))
        }
    }
}
