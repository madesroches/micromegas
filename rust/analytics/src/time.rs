use chrono::TimeDelta;
use micromegas_telemetry::types::process::Process;

#[derive(Debug, Clone)]
pub struct ConvertTicks {
    ts_offset: i64,
    frequency: i64, // ticks per second
    inv_tsc_frequency: f64,
}

impl ConvertTicks {
    pub fn new(process: &Process) -> Self {
        Self::from_ticks(process.start_ticks, process.tsc_frequency)
    }

    pub fn from_ticks(start_ticks: i64, frequency: i64) -> Self {
        let inv_tsc_frequency = get_tsc_frequency_inverse_ms(frequency);
        Self {
            ts_offset: start_ticks,
            frequency,
            inv_tsc_frequency,
        }
    }

    pub fn from_meta_data(start_ticks: i64, frequency: i64) -> Self {
        Self {
            ts_offset: start_ticks,
            frequency,
            inv_tsc_frequency: get_tsc_frequency_inverse_ms(frequency),
        }
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn get_time(&self, ts: i64) -> f64 {
        (ts - self.ts_offset) as f64 * self.inv_tsc_frequency
    }

    pub fn to_ticks(&self, delta: TimeDelta) -> i64 {
        const NANOS_PER_SEC: f64 = 1000.0 * 1000.0 * 1000.0;
        let mut seconds = delta.num_seconds() as f64;
        seconds += delta.subsec_nanos() as f64 / NANOS_PER_SEC;
        let freq = self.frequency as f64;
        (seconds * freq).round() as i64
    }
}

pub fn get_process_tick_length_ms(process_info: &Process) -> f64 {
    get_tsc_frequency_inverse_ms(process_info.tsc_frequency)
}

#[allow(clippy::cast_precision_loss)]
pub fn get_tsc_frequency_inverse_ms(tsc_frequency: i64) -> f64 {
    1000.0 / tsc_frequency as f64
}
