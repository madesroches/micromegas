use chrono::prelude::*;
use std::sync::atomic::{AtomicI64, Ordering};

use crate::dispatch::{flush_log_buffer, flush_metrics_buffer, for_each_thread_stream, get_sink};

// FlushMonitor triggers the flush of the telemetry streams every minute.
//   Must be ticked.
//   Will not flush the streams if the sink is busy writing.
//   Thread streams can't be flushed without introducing a synchronization mechanism. Their capacity is reduced so that the calling code will flush them in a safe manner.
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
            if let Some(sink) = get_sink() {
                if sink.is_busy() {
                    return;
                }
            } else {
                return;
            }
            self.last_flush
                .store(Local::now().timestamp(), Ordering::Relaxed);
            flush_log_buffer();
            flush_metrics_buffer();
            for_each_thread_stream(&mut |stream_ptr| unsafe {
                (*stream_ptr).set_full();
            });
        }
    }
}

impl Default for FlushMonitor {
    fn default() -> Self {
        // Default is to flush every minute
        Self::new(60)
    }
}
