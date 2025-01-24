//! FlushMonitor triggers the flush of the telemetry streams at regular interval.
use chrono::prelude::*;
use std::sync::atomic::{AtomicI64, Ordering};

use crate::dispatch::{flush_log_buffer, flush_metrics_buffer, for_each_thread_stream};

pub struct FlushMonitor {
    last_flush: AtomicI64,
    flush_period_seconds: i64,
}

impl FlushMonitor {
    pub fn new(flush_period_seconds: i64) -> Self {
        Self {
            last_flush: AtomicI64::new(Local::now().timestamp()),
            flush_period_seconds,
        }
    }

    pub fn time_to_flush_seconds(&self) -> i64 {
        let now = Local::now().timestamp();
        let seconds_since_flush = now - self.last_flush.load(Ordering::Relaxed);
        self.flush_period_seconds - seconds_since_flush
    }

    pub fn tick(&self) {
        if self.time_to_flush_seconds() <= 0 {
            self.last_flush
                .store(Local::now().timestamp(), Ordering::Relaxed);
            flush_log_buffer();
            flush_metrics_buffer();
            for_each_thread_stream(&mut |stream_ptr| unsafe {
                //Thread streams can't be flushed without introducing a synchronization mechanism. They are marked as full so that the calling code will flush them in a safe manner.
                (*stream_ptr).set_full();
            });
        }
    }
}

impl Default for FlushMonitor {
    fn default() -> Self {
        // Default is to flush every minute unless specified by the env variable
        const DEFAULT_PERIOD: i64 = 60;
        let nb_seconds = std::env::var("MICROMEGAS_FLUSH_PERIOD")
            .map(|v| v.parse::<i64>().unwrap_or(DEFAULT_PERIOD))
            .unwrap_or(DEFAULT_PERIOD);
        Self::new(nb_seconds)
    }
}
