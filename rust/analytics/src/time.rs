use micromegas_tracing::ProcessInfo;

#[derive(Debug, Clone)]
pub struct ConvertTicks {
    ts_offset: i64,
    inv_tsc_frequency: f64,
}

impl ConvertTicks {
    pub fn new(process: &ProcessInfo) -> Self {
        let inv_tsc_frequency = get_process_tick_length_ms(process);
        Self {
            ts_offset: process.start_ticks,
            inv_tsc_frequency,
        }
    }

    pub fn from_meta_data(start_ticks: i64, tsc_frequency: u64) -> Self {
        Self {
            ts_offset: start_ticks,
            inv_tsc_frequency: get_tsc_frequency_inverse_ms(tsc_frequency),
        }
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn get_time(&self, ts: i64) -> f64 {
        (ts - self.ts_offset) as f64 * self.inv_tsc_frequency
    }
}

pub fn get_process_tick_length_ms(process_info: &ProcessInfo) -> f64 {
    get_tsc_frequency_inverse_ms(process_info.tsc_frequency)
}

#[allow(clippy::cast_precision_loss)]
pub fn get_tsc_frequency_inverse_ms(tsc_frequency: u64) -> f64 {
    1000.0 / tsc_frequency as f64
}
