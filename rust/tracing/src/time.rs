//! System & monotonic tick count
use chrono::{DateTime, Utc};

#[derive(Debug)]
pub struct DualTime {
    pub ticks: i64,
    pub time: DateTime<Utc>,
}

impl DualTime {
    pub fn now() -> Self {
        Self {
            ticks: now(),
            time: Utc::now(),
        }
    }
}

#[allow(clippy::cast_possible_wrap)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn now() -> i64 {
    //_rdtsc does not wait for previous instructions to be retired
    // we could use __rdtscp if we needed more precision at the cost of slightly
    // higher overhead
    use core::arch::x86_64::_rdtsc;
    unsafe { _rdtsc() as i64 }
}

#[allow(clippy::cast_possible_wrap)]
#[cfg(target_arch = "aarch64")]
pub fn now() -> i64 {
    //essentially from https://github.com/sheroz/tick_counter/blob/main/src/lib.rs
    //(MIT license)
    let tick_counter: i64;
    unsafe {
        core::arch::asm!(
            "mrs x0, cntvct_el0",
            out("x0") tick_counter
        );
    }
    tick_counter
}

pub fn frequency() -> u64 {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        let cpuid = raw_cpuid::CpuId::new();
        if let Some(Some(frequency)) = cpuid
            .get_tsc_info()
            .map(|tsc_info| tsc_info.tsc_frequency())
        {
            frequency
        } else {
            // For the fallbacks here, performed some tests on multiple configuration
            // and found that the following values are the most accurate when we fail
            // to get the frequency from the CPUID.
            // Linux is more accurate with the information from the cpuinfo file
            frequency_fallback()
        }
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    frequency_fallback()
}

#[cfg(windows)]
#[allow(clippy::manual_c_str_literals)]
fn frequency_fallback() -> u64 {
    // https://www.codeproject.com/Articles/7340/Get-the-Processor-Speed-in-two-simple-ways
    #[link(name = "kernel32")]
    extern "system" {
        // https://docs.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-regopenkeyexa
        fn RegOpenKeyExA(
            key_handle: u64,
            sub_key: *const u8,
            options: u32,
            sam_desired: u32,
            key_handle_result: *mut u64,
        ) -> u32;
        // https://docs.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-regqueryvalueexa
        fn RegQueryValueExA(
            key_handle: u64,
            value_name: *const u8,
            reserved: *mut u32,
            value_type: *mut u32,
            data: *mut u8,
            data_size: *mut u32,
        ) -> u32;
    }
    // (STANDARD_RIGHTS_READ | KEY_QUERY_VALUE | KEY_ENUMERATE_SUB_KEYS | KEY_NOTIFY) & ~SYNCHRONIZE
    const KEY_READ: u32 = 0x00020019;
    const HKEY_LOCAL_MACHINE: u64 = 0x80000002;
    #[allow(unsafe_code)]
    unsafe {
        let mut key_handle_result = 0;
        if RegOpenKeyExA(
            HKEY_LOCAL_MACHINE,
            b"HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0\0".as_ptr(),
            0,
            KEY_READ,
            &mut key_handle_result,
        ) == 0
        {
            let mut frequency: u64 = 0;
            let mut data_size = std::mem::size_of_val(&frequency) as u32;
            if RegQueryValueExA(
                key_handle_result,
                b"~MHz\0".as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::addr_of_mut!(frequency).cast::<u8>(),
                &mut data_size,
            ) == 0
            {
                frequency * 1_000_000
            } else {
                0
            }
        } else {
            0
        }
    }
}

#[cfg(target_os = "linux")]
fn frequency_fallback() -> u64 {
    // Simpler method not requiring super user nor a kernel module
    // Accuracy was tested on a couple of VMs
    // https://stackoverflow.com/questions/35123379/getting-tsc-rate-from-x86-kernel
    // https://blog.trailofbits.com/2019/10/03/tsc-frequency-for-all-better-profiling-and-benchmarking/
    // https://stackoverflow.com/questions/51919219/determine-tsc-frequency-on-linux
    if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
        (cpuinfo
            .lines()
            .filter(|line| line.starts_with("cpu MHz"))
            .map(|line| line.split(':').nth(1).unwrap().trim().parse::<f64>())
            .find(std::result::Result::is_ok)
            .unwrap_or(Ok(0.0))
            .unwrap_or(0.0)
            * 1_000_000.0) as u64
    } else {
        0
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn frequency_fallback() -> u64 {
    0
}

#[allow(unused_imports)]
#[cfg(test)]
mod tests {
    use crate::time::frequency;

    #[test]
    fn test_frequency() {
        assert!(frequency() > 0);
    }
}
