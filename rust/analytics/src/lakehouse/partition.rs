use super::view::ViewMetadata;
use crate::time::TimeRange;
use chrono::{DateTime, Utc};

/// Partition metadata (without embedded file_metadata for performance)
/// Use load_partition_metadata() to load metadata on-demand when needed
#[derive(Clone, Debug)]
pub struct Partition {
    /// Metadata about the view this partition belongs to.
    pub view_metadata: ViewMetadata,
    /// The insert time range for this partition.
    pub insert_time_range: TimeRange,
    /// The event time range for this partition. None for empty partitions.
    pub event_time_range: Option<TimeRange>,
    /// The last time this partition was updated.
    pub updated: DateTime<Utc>,
    /// The path to the Parquet file for this partition. None for empty partitions.
    pub file_path: Option<String>,
    /// The size of the Parquet file in bytes. 0 for empty partitions.
    pub file_size: i64,
    /// A hash of the source data that generated this partition.
    pub source_data_hash: Vec<u8>,
    /// The number of rows in this partition. 0 for empty partitions.
    pub num_rows: i64,
}

impl Partition {
    /// Returns true if this partition has no data (num_rows = 0).
    pub fn is_empty(&self) -> bool {
        self.num_rows == 0
    }

    /// Returns the min event time, if this partition has data.
    pub fn min_event_time(&self) -> Option<DateTime<Utc>> {
        self.event_time_range.as_ref().map(|r| r.begin)
    }

    /// Returns the max event time, if this partition has data.
    pub fn max_event_time(&self) -> Option<DateTime<Utc>> {
        self.event_time_range.as_ref().map(|r| r.end)
    }

    /// Returns the beginning of the insert time range.
    pub fn begin_insert_time(&self) -> DateTime<Utc> {
        self.insert_time_range.begin
    }

    /// Returns the end of the insert time range.
    pub fn end_insert_time(&self) -> DateTime<Utc> {
        self.insert_time_range.end
    }

    /// Validates partition invariants. Returns error if partition is inconsistent.
    ///
    /// Invariants:
    /// - Non-empty partitions (num_rows > 0) MUST have both event_time_range and file_path
    /// - Empty partitions (num_rows = 0) MUST NOT have event_time_range or file_path
    /// - num_rows must not be negative
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.num_rows > 0 {
            // Non-empty partition must have event_time_range and file_path
            if self.event_time_range.is_none() {
                anyhow::bail!(
                    "non-empty partition (num_rows={}) has no event_time_range",
                    self.num_rows
                );
            }
            if self.file_path.is_none() {
                anyhow::bail!(
                    "non-empty partition (num_rows={}) has no file_path",
                    self.num_rows
                );
            }
        } else if self.num_rows == 0 {
            // Empty partition must NOT have event_time_range or file_path
            if self.event_time_range.is_some() {
                anyhow::bail!("empty partition has event_time_range");
            }
            if self.file_path.is_some() {
                anyhow::bail!("empty partition has file_path");
            }
        } else {
            anyhow::bail!("partition has negative num_rows: {}", self.num_rows);
        }
        Ok(())
    }
}
