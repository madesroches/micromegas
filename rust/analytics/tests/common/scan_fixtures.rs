//! Offline (no DB) `Partition`/`ReaderFactory` builders for tests that plan partition scans
//! without touching storage. Not every caller uses every helper.
#![allow(dead_code)]

use chrono::{TimeDelta, Utc};
use micromegas_analytics::lakehouse::metadata_cache::MetadataCache;
use micromegas_analytics::lakehouse::partition::Partition;
use micromegas_analytics::lakehouse::reader_factory::ReaderFactory;
use micromegas_analytics::lakehouse::view::ViewMetadata;
use micromegas_analytics::time::TimeRange;
use std::sync::Arc;

pub fn make_reader_factory(cache: Arc<MetadataCache>) -> Arc<ReaderFactory> {
    Arc::new(ReaderFactory::new(
        Arc::new(object_store::memory::InMemory::new()),
        cache,
    ))
}

/// Distinct, non-overlapping event-time ranges per `index`, so the same partitions are valid
/// inputs for a `Concatenated` scan as well as an arbitrarily-overlapping `PerFile` one.
pub fn make_partition(
    file_path: &str,
    index: i64,
    sort_order: Option<Vec<String>>,
    file_size: i64,
) -> Partition {
    let t0 = Utc::now() + TimeDelta::seconds(index * 10);
    Partition {
        view_metadata: ViewMetadata {
            view_set_name: Arc::new("metrics".to_owned()),
            view_instance_id: Arc::new("global".to_owned()),
            file_schema_hash: vec![0],
        },
        insert_time_range: TimeRange::new(t0, t0 + TimeDelta::seconds(10)),
        event_time_range: Some(TimeRange::new(t0, t0 + TimeDelta::seconds(10))),
        updated: t0,
        file_path: Some(file_path.to_owned()),
        file_size,
        source_data_hash: vec![0],
        num_rows: 10,
        sort_order,
        max_sort_key_time: None,
    }
}

pub fn make_empty_partition(index: i64) -> Partition {
    let t0 = Utc::now() + TimeDelta::seconds(index * 10);
    Partition {
        view_metadata: ViewMetadata {
            view_set_name: Arc::new("metrics".to_owned()),
            view_instance_id: Arc::new("global".to_owned()),
            file_schema_hash: vec![0],
        },
        insert_time_range: TimeRange::new(t0, t0 + TimeDelta::seconds(10)),
        event_time_range: None,
        updated: t0,
        file_path: None,
        file_size: 0,
        source_data_hash: vec![0],
        num_rows: 0,
        sort_order: None,
        max_sort_key_time: None,
    }
}
