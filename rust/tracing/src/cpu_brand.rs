//! Host CPU model identification.

/// Human-readable identification of the host CPU model.
pub fn get_cpu_brand() -> String {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    return raw_cpuid::CpuId::new()
        .get_processor_brand_string()
        .map_or_else(|| "unknown".to_owned(), |b| b.as_str().to_owned());
    #[cfg(all(
        target_arch = "aarch64",
        any(target_os = "linux", target_os = "android")
    ))]
    return read_arm_brand().unwrap_or_else(|| std::env::consts::ARCH.to_owned());
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    return macos_brand_string().unwrap_or_else(|| std::env::consts::ARCH.to_owned());
    #[cfg(not(any(
        target_arch = "x86",
        target_arch = "x86_64",
        all(
            target_arch = "aarch64",
            any(target_os = "linux", target_os = "android", target_os = "macos")
        )
    )))]
    return std::env::consts::ARCH.to_owned();
}

#[cfg(all(
    target_arch = "aarch64",
    any(target_os = "linux", target_os = "android")
))]
fn read_arm_brand() -> Option<String> {
    // /proc/cpuinfo is the fallback for kernels or sandboxes that hide the regs directory.
    let (implementer, part) =
        std::fs::read_to_string("/sys/devices/system/cpu/cpu0/regs/identification/midr_el1")
            .ok()
            .and_then(|t| parse_midr(&t))
            .or_else(|| {
                std::fs::read_to_string("/proc/cpuinfo")
                    .ok()
                    .and_then(|t| parse_cpuinfo_ids(&t))
            })?;
    Some(format_arm_brand(implementer, part))
}

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
fn macos_brand_string() -> Option<String> {
    use std::ffi::{CString, c_char, c_int, c_void};

    unsafe extern "C" {
        fn sysctlbyname(
            name: *const c_char,
            oldp: *mut c_void,
            oldlenp: *mut usize,
            newp: *mut c_void,
            newlen: usize,
        ) -> c_int;
    }

    let name = CString::new("machdep.cpu.brand_string").ok()?;
    let mut len: usize = 0;
    // SAFETY: a null buffer asks sysctl only for the required size.
    let rc = unsafe {
        sysctlbyname(
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len];
    // SAFETY: buf has exactly `len` writable bytes.
    let rc = unsafe {
        sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast::<c_void>(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    buf.truncate(len);
    while buf.last() == Some(&0) {
        buf.pop();
    }
    let brand = String::from_utf8_lossy(&buf).trim().to_owned();
    (!brand.is_empty()).then_some(brand)
}

#[cfg_attr(
    not(all(
        target_arch = "aarch64",
        any(target_os = "linux", target_os = "android")
    )),
    allow(dead_code)
)]
fn parse_midr(text: &str) -> Option<(u32, u32)> {
    let t = text.trim();
    let t = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .unwrap_or(t);
    let midr = u64::from_str_radix(t, 16).ok()?;
    Some((((midr >> 24) & 0xff) as u32, ((midr >> 4) & 0xfff) as u32))
}

#[cfg_attr(
    not(all(
        target_arch = "aarch64",
        any(target_os = "linux", target_os = "android")
    )),
    allow(dead_code)
)]
fn parse_cpuinfo_ids(text: &str) -> Option<(u32, u32)> {
    let mut implementer = None;
    let mut part = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let parse_hex = |v: &str| {
            let v = v.trim();
            u32::from_str_radix(v.strip_prefix("0x").unwrap_or(v), 16).ok()
        };
        match key.trim() {
            "CPU implementer" if implementer.is_none() => implementer = parse_hex(value),
            "CPU part" if part.is_none() => part = parse_hex(value),
            _ => {}
        }
        if implementer.is_some() && part.is_some() {
            break;
        }
    }
    Some((implementer?, part?))
}

#[cfg_attr(
    not(all(
        target_arch = "aarch64",
        any(target_os = "linux", target_os = "android")
    )),
    allow(dead_code)
)]
fn format_arm_brand(implementer: u32, part: u32) -> String {
    match arm_part_name(implementer, part) {
        Some(name) => format!("{name} (0x{implementer:02x}:0x{part:03x})"),
        None => format!("0x{implementer:02x}:0x{part:03x}"),
    }
}

#[cfg_attr(
    not(all(
        target_arch = "aarch64",
        any(target_os = "linux", target_os = "android")
    )),
    allow(dead_code)
)]
fn arm_part_name(implementer: u32, part: u32) -> Option<&'static str> {
    match (implementer, part) {
        (0x41, 0xd03) => Some("Cortex-A53"),
        (0x41, 0xd05) => Some("Cortex-A55"),
        (0x41, 0xd07) => Some("Cortex-A57"),
        (0x41, 0xd08) => Some("Cortex-A72"),
        (0x41, 0xd09) => Some("Cortex-A73"),
        (0x41, 0xd0a) => Some("Cortex-A75"),
        (0x41, 0xd0b) => Some("Cortex-A76"),
        (0x41, 0xd0c) => Some("Neoverse-N1"),
        (0x41, 0xd0d) => Some("Cortex-A77"),
        (0x41, 0xd40) => Some("Neoverse-V1"),
        (0x41, 0xd41) => Some("Cortex-A78"),
        (0x41, 0xd44) => Some("Cortex-X1"),
        (0x41, 0xd46) => Some("Cortex-A510"),
        (0x41, 0xd47) => Some("Cortex-A710"),
        (0x41, 0xd48) => Some("Cortex-X2"),
        (0x41, 0xd49) => Some("Neoverse-N2"),
        (0x41, 0xd4f) => Some("Neoverse-V2"),
        (0x41, 0xd84) => Some("Neoverse-V3"),
        (0x41, 0xd8e) => Some("Neoverse-N3"),
        (0xc0, 0xac3) => Some("Ampere-1"),
        (0xc0, 0xac4) => Some("Ampere-1a"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midr_parsing() {
        assert_eq!(parse_midr("0x00000000413fd0c1"), Some((0x41, 0xd0c)));
        assert_eq!(parse_midr("0x00000000413fd0c1\n"), Some((0x41, 0xd0c)));
        assert_eq!(parse_midr("00000000413fd0c1"), Some((0x41, 0xd0c)));
        assert_eq!(parse_midr(""), None);
        assert_eq!(parse_midr("garbage"), None);
    }

    #[test]
    fn cpuinfo_parsing() {
        let arm = "processor\t: 0\nBogoMIPS\t: 243.75\nFeatures\t: fp asimd\n\
                   CPU implementer\t: 0x41\nCPU architecture: 8\nCPU variant\t: 0x3\n\
                   CPU part\t: 0xd0c\nCPU revision\t: 1\n\n\
                   processor\t: 1\nCPU implementer\t: 0x61\nCPU part\t: 0x022\n";
        assert_eq!(parse_cpuinfo_ids(arm), Some((0x41, 0xd0c)));
        assert_eq!(
            parse_cpuinfo_ids("processor : 0\nCPU implementer : 0x41\n"),
            None
        );
        let x86 = "processor\t: 0\nvendor_id\t: GenuineIntel\nmodel name\t: Xeon\n";
        assert_eq!(parse_cpuinfo_ids(x86), None);
    }

    #[test]
    fn brand_formatting() {
        assert_eq!(format_arm_brand(0x41, 0xd0c), "Neoverse-N1 (0x41:0xd0c)");
        assert_eq!(format_arm_brand(0x41, 0xd40), "Neoverse-V1 (0x41:0xd40)");
        assert_eq!(format_arm_brand(0x41, 0xd4f), "Neoverse-V2 (0x41:0xd4f)");
        assert_eq!(format_arm_brand(0x41, 0xfff), "0x41:0xfff");
        assert_eq!(format_arm_brand(0x61, 0x022), "0x61:0x022");
    }

    #[test]
    fn host_brand_is_not_empty() {
        assert!(!get_cpu_brand().is_empty());
    }
}
