use chrono::TimeDelta;
use micromegas_tracing::process_info::ProcessInfo;

const NANOS_PER_SEC: f64 = 1000.0 * 1000.0 * 1000.0;

#[derive(Debug, Clone)]
pub struct ConvertTicks {
    tick_offset: i64,
    process_start_ns: i64,
    frequency: i64, // ticks per second
    inv_tsc_frequency_ns: f64,
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
        }
    }

    pub fn to_ticks(&self, delta: TimeDelta) -> i64 {
        let mut seconds = delta.num_seconds() as f64;
        seconds += delta.subsec_nanos() as f64 / NANOS_PER_SEC;
        let freq = self.frequency as f64;
        (seconds * freq).round() as i64
    }

    pub fn ticks_to_nanoseconds(&self, ticks: i64) -> i64 {
        let delta = (ticks - self.tick_offset) as f64;
        let ns_since_process_start = (delta * self.inv_tsc_frequency_ns).round() as i64;
        self.process_start_ns + ns_since_process_start
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
