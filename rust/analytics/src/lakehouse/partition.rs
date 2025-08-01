use std::sync::Arc;

use super::view::ViewMetadata;
use chrono::{DateTime, Utc};
use datafusion::parquet::file::metadata::ParquetMetaData;

/// Partition metadata
#[derive(Clone, Debug)]
pub struct Partition {
    /// Metadata about the view this partition belongs to.
    pub view_metadata: ViewMetadata,
    /// The inclusive beginning of the insert time range for this partition.
    pub begin_insert_time: DateTime<Utc>,
    /// The exclusive end of the insert time range for this partition.
    pub end_insert_time: DateTime<Utc>,
    /// The minimum event time contained in this partition.
    pub min_event_time: DateTime<Utc>,
    /// The maximum event time contained in this partition.
    pub max_event_time: DateTime<Utc>,
    /// The last time this partition was updated.
    pub updated: DateTime<Utc>,
    /// The path to the Parquet file for this partition.
    pub file_path: String,
    /// The size of the Parquet file in bytes.
    pub file_size: i64,
    /// A hash of the source data that generated this partition.
    pub source_data_hash: Vec<u8>,
    /// The Parquet file metadata.
    pub file_metadata: Arc<ParquetMetaData>,
}
