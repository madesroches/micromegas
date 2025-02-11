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

#[cfg(windows)]
pub fn now_windows() -> i64 {
    unsafe {
        let mut tick_count = std::mem::zeroed();
        winapi::um::profileapi::QueryPerformanceCounter(&mut tick_count);
        *tick_count.QuadPart() as i64
    }
}

#[cfg(windows)]
pub fn freq_windows() -> i64 {
    unsafe {
        let mut tick_count = std::mem::zeroed();
        winapi::um::profileapi::QueryPerformanceFrequency(&mut tick_count);
        *tick_count.QuadPart() as i64
    }
}

#[allow(unreachable_code, clippy::cast_possible_wrap)]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn now() -> i64 {
    #[cfg(windows)]
    return now_windows();

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

#[allow(unreachable_code)]
pub fn frequency() -> i64 {
    #[cfg(windows)]
    return freq_windows();

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        let cpuid = raw_cpuid::CpuId::new();
        return cpuid
            .get_tsc_info()
            .map(|tsc_info| tsc_info.tsc_frequency().unwrap_or(0))
            .unwrap_or(0) as i64;
    }
    #[cfg(target_arch = "aarch64")]
    {
        let counter_frequency: i64;
        unsafe {
            core::arch::asm!(
                "mrs x0, cntfrq_el0",
                out("x0") counter_frequency
            );
        }
        return counter_frequency;
    }
    0
}

#[allow(unused_imports)]
#[cfg(test)]
mod tests {
    use crate::time::frequency;

    #[test]
    fn test_frequency() {
        eprintln!("cpu frequency: {}", frequency());
    }
}
