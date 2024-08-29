use chrono::{DateTime, TimeDelta, Utc};
use micromegas_tracing::process_info::ProcessInfo;

const NANOS_PER_SEC: f64 = 1000.0 * 1000.0 * 1000.0;

/// ConvertTicks helps converting between a process's tick count and more convenient date/time representations
#[derive(Debug, Clone)]
pub struct ConvertTicks {
    tick_offset: i64,
    process_start_ns: i64,
    frequency: i64, // ticks per second
    inv_tsc_frequency_ns: f64,
    inv_tsc_frequency_ms: f64,
}

impl ConvertTicks {
    pub fn new(process: &ProcessInfo) -> Self {
        Self::from_meta_data(
            process.start_ticks,
            process.start_time.timestamp_nanos_opt().unwrap_or_default(),
            process.tsc_frequency,
        )
    }

    pub fn from_meta_data(start_ticks: i64, process_start_ns: i64, frequency: i64) -> Self {
        Self {
            tick_offset: start_ticks,
            process_start_ns,
            frequency,
            inv_tsc_frequency_ns: get_tsc_frequency_inverse_ns(frequency),
            inv_tsc_frequency_ms: get_tsc_frequency_inverse_ms(frequency),
        }
    }

    /// from relative time to relative tick count
    pub fn to_ticks(&self, delta: TimeDelta) -> i64 {
        let mut seconds = delta.num_seconds() as f64;
        seconds += delta.subsec_nanos() as f64 / NANOS_PER_SEC;
        let freq = self.frequency as f64;
        (seconds * freq).round() as i64
    }

    /// from absolute ticks to absolute nanoseconds
    pub fn ticks_to_nanoseconds(&self, ticks: i64) -> i64 {
        let delta = (ticks - self.tick_offset) as f64;
        let ns_since_process_start = (delta * self.inv_tsc_frequency_ns).round() as i64;
        self.process_start_ns + ns_since_process_start
    }

    /// from relative ticks to absolute date/time
    pub fn delta_ticks_to_time(&self, delta: i64) -> DateTime<Utc> {
        let ns_since_process_start = (delta as f64 * self.inv_tsc_frequency_ns).round() as i64;
        DateTime::from_timestamp_nanos(self.process_start_ns + ns_since_process_start)
    }

    /// from relative ticks to absolute nanoseconds
    pub fn delta_ticks_to_ns(&self, delta: i64) -> i64 {
        let ns_since_process_start = (delta as f64 * self.inv_tsc_frequency_ns).round() as i64;
        self.process_start_ns + ns_since_process_start
    }

    /// from relative ticks to relative milliseconds
    pub fn delta_ticks_to_ms(&self, delta_ticks: i64) -> f64 {
        let delta = delta_ticks as f64;
        delta * self.inv_tsc_frequency_ms
    }
}

#[allow(clippy::cast_precision_loss)]
pub fn get_tsc_frequency_inverse_ms(tsc_frequency: i64) -> f64 {
    1000.0 / tsc_frequency as f64
}

#[allow(clippy::cast_precision_loss)]
pub fn get_tsc_frequency_inverse_ns(tsc_frequency: i64) -> f64 {
    NANOS_PER_SEC / tsc_frequency as f64
}
