use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use datafusion::scalar::ScalarValue;
use micromegas_telemetry::types::block::BlockMetadata;
use sqlx::Row;

use crate::metadata::ProcessMetadata;

const NANOS_PER_SEC: f64 = 1000.0 * 1000.0 * 1000.0;

/// A time range, with a beginning and an end.
#[derive(Clone, Copy, Debug)]
pub struct TimeRange {
    pub begin: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl TimeRange {
    pub fn new(begin: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        Self { begin, end }
    }
}

/// Creates a `ConvertTicks` from a database connection.
pub async fn make_time_converter_from_db(
    pool: &sqlx::Pool<sqlx::Postgres>,
    process: &ProcessMetadata,
) -> Result<ConvertTicks> {
    if process.tsc_frequency > 0 {
        // we have a good tsc freq provided
        return ConvertTicks::from_meta_data(
            process.start_ticks,
            process.start_time.timestamp_nanos_opt().unwrap_or_default(),
            process.tsc_frequency,
        );
    }
    // we need to estimate the tsc frequency
    let row = sqlx::query(
        "SELECT end_time, end_ticks
         FROM blocks
         WHERE process_id = $1
         ORDER BY end_time DESC
         LIMIT 1",
    )
    .bind(process.process_id)
    .fetch_one(pool)
    .await
    .with_context(|| "getting last block end time for tsc estimation")?;
    let end_time: chrono::DateTime<chrono::Utc> = row.try_get("end_time")?;
    let relative_end_ticks: i64 = row.try_get("end_ticks")?;
    let delta_time = end_time - process.start_time;
    let nb_seconds = delta_time.num_nanoseconds().unwrap_or_default() as f64 / 1_000_000_000.0;
    let ticks_per_second = relative_end_ticks as f64 / nb_seconds;
    ConvertTicks::from_meta_data(
        process.start_ticks,
        process.start_time.timestamp_nanos_opt().unwrap_or_default(),
        ticks_per_second.round() as i64,
    )
}

/// Creates a `ConvertTicks` from a block's metadata.
pub fn make_time_converter_from_block_meta(
    process: &ProcessMetadata,
    block: &BlockMetadata,
) -> Result<ConvertTicks> {
    if process.tsc_frequency > 0 {
        // we have a good tsc freq provided
        return ConvertTicks::from_meta_data(
            process.start_ticks,
            process.start_time.timestamp_nanos_opt().unwrap_or_default(),
            process.tsc_frequency,
        );
    }
    let delta_time = block.end_time - process.start_time;
    let nb_seconds = delta_time.num_nanoseconds().unwrap_or_default() as f64 / 1_000_000_000.0;
    let ticks_per_second = block.end_ticks as f64 / nb_seconds;
    ConvertTicks::from_meta_data(
        process.start_ticks,
        process.start_time.timestamp_nanos_opt().unwrap_or_default(),
        ticks_per_second.round() as i64,
    )
}

/// Creates a `ConvertTicks` using the latest timing information from the process.
/// This should be used instead of per-block timing to ensure consistent tick conversion
/// across all blocks from the same process.
pub fn make_time_converter_from_latest_timing(
    process: &ProcessMetadata,
    last_block_end_ticks: i64,
    last_block_end_time: chrono::DateTime<chrono::Utc>,
) -> Result<ConvertTicks> {
    if process.tsc_frequency > 0 {
        // we have a good tsc freq provided
        return ConvertTicks::from_meta_data(
            process.start_ticks,
            process.start_time.timestamp_nanos_opt().unwrap_or_default(),
            process.tsc_frequency,
        );
    }
    // Calculate frequency using the latest timing data from the process
    let delta_time = last_block_end_time - process.start_time;
    let nb_seconds = delta_time.num_nanoseconds().unwrap_or_default() as f64 / 1_000_000_000.0;
    let ticks_per_second = last_block_end_ticks as f64 / nb_seconds;
    ConvertTicks::from_meta_data(
        process.start_ticks,
        process.start_time.timestamp_nanos_opt().unwrap_or_default(),
        ticks_per_second.round() as i64,
    )
}

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
    pub fn from_meta_data(start_ticks: i64, process_start_ns: i64, frequency: i64) -> Result<Self> {
        if frequency <= 0 {
            anyhow::bail!("invalid frequency")
        }
        Ok(Self {
            tick_offset: start_ticks,
            process_start_ns,
            frequency,
            inv_tsc_frequency_ns: get_tsc_frequency_inverse_ns(frequency),
            inv_tsc_frequency_ms: get_tsc_frequency_inverse_ms(frequency),
        })
    }

    /// Get the frequency used for tick conversion
    pub fn get_frequency(&self) -> i64 {
        self.frequency
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

    /// from time to relative ticks
    pub fn time_to_delta_ticks(&self, time: DateTime<Utc>) -> i64 {
        self.to_ticks(time - DateTime::from_timestamp_nanos(self.process_start_ns))
    }
}

/// Returns the inverse of the TSC frequency in milliseconds.
#[allow(clippy::cast_precision_loss)]
pub fn get_tsc_frequency_inverse_ms(tsc_frequency: i64) -> f64 {
    1000.0 / tsc_frequency as f64
}

/// Returns the inverse of the TSC frequency in nanoseconds.
#[allow(clippy::cast_precision_loss)]
pub fn get_tsc_frequency_inverse_ns(tsc_frequency: i64) -> f64 {
    NANOS_PER_SEC / tsc_frequency as f64
}

/// Converts a `DateTime<Utc>` to a `ScalarValue`.
pub fn datetime_to_scalar(v: DateTime<Utc>) -> ScalarValue {
    lazy_static::lazy_static! {
        static ref UTC_OFFSET: Arc<str> = Arc::from("+00:00");
    }
    ScalarValue::TimestampNanosecond(v.timestamp_nanos_opt(), Some(UTC_OFFSET.clone()))
}
