use anyhow::{Context, Result};
use std::{ffi::OsString, os::windows::ffi::OsStrExt};
use windows::{
    core::{w, PCWSTR},
    Win32::System::Registry::{
        RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    },
};

#[derive(Debug)]
pub struct WideString {
    inner: Vec<u16>,
}

impl WideString {
    pub fn as_bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                self.inner.as_ptr() as *const u8,
                self.inner.len() * std::mem::size_of::<u16>(),
            )
        }
    }

    pub fn as_pcwstr(&self) -> PCWSTR {
        PCWSTR(self.inner.as_ptr())
    }
}

impl From<&str> for WideString {
    fn from(value: &str) -> Self {
        let mut array: Vec<u16> = OsString::from(value).encode_wide().collect();
        array.push(0); // encode_wide does not guarantee a null terminator
        Self { inner: array }
    }
}

pub unsafe fn install() -> Result<()> {
    println!("installing");
    let key_path = w!(r"Software\Classes\micromegas");
    let current_exe = std::path::absolute(
        std::env::args()
            .next()
            .with_context(|| "reading exe path from args")?,
    )?
    .to_str()
    .with_context(|| "converting exe path to utf8")?
    .to_owned();
    let hkey: HKEY = HKEY_CURRENT_USER;
    let mut key_handle = HKEY::default();

    RegCreateKeyExW(
        hkey,
        key_path,
        None,
        None,
        REG_OPTION_NON_VOLATILE,
        KEY_WRITE,
        None,
        &mut key_handle,
        None,
    )
    .ok()
    .with_context(|| "RegCreateKeyExW")?;

    let value_name = w!("");
    let value_data = WideString::from("URL:micromegas Protocol");
    RegSetValueExW(
        key_handle,
        value_name,
        None,
        REG_SZ,
        Some(value_data.as_bytes()),
    )
    .ok()?;

    let value_name = w!("URL Protocol");
    let value_data = WideString::from("");
    RegSetValueExW(
        key_handle,
        value_name,
        None,
        REG_SZ,
        Some(value_data.as_bytes()),
    )
    .ok()?;

    let command = WideString::from(format!(r#""{current_exe}" "%1""#).as_str());
    let command_key_path = WideString::from(r"Software\Classes\micromegas\shell\open\command");
    RegCreateKeyExW(
        hkey,
        command_key_path.as_pcwstr(),
        None,
        None,
        REG_OPTION_NON_VOLATILE,
        KEY_WRITE,
        None,
        &mut key_handle,
        None,
    )
    .ok()?;
    let value_name = w!("");
    RegSetValueExW(
        key_handle,
        value_name,
        None,
        REG_SZ,
        Some(command.as_bytes()),
    )
    .ok()?;

    Ok(())
}
