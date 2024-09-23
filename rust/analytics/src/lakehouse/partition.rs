use std::sync::Arc;

use super::view::ViewMetadata;
use chrono::{DateTime, Utc};
use datafusion::parquet::file::metadata::ParquetMetaData;

/// Partition metadata
#[derive(Clone, Debug)]
pub struct Partition {
    pub view_metadata: ViewMetadata,
    pub begin_insert_time: DateTime<Utc>,
    pub end_insert_time: DateTime<Utc>,
    pub min_event_time: DateTime<Utc>,
    pub max_event_time: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    pub file_path: String,
    pub file_size: i64,
    pub source_data_hash: Vec<u8>,
    pub file_metadata: Arc<ParquetMetaData>,
}
