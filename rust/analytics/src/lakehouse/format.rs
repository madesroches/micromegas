//! `streams.format` literals — the keys used for per-block processor dispatch.
//!
//! Both `BlockPartitionSpec::block_processors` and the JIT
//! `write_partition_from_blocks` map are keyed by these strings; views register
//! one entry per format they understand. The constants live in
//! `micromegas-ingestion` (writer side) and are re-exported here so reader and
//! writer share a single source of truth.

pub use micromegas_ingestion::web_ingestion_service::{
    FORMAT_OTLP_LOGS, FORMAT_OTLP_METRICS, FORMAT_OTLP_TRACES, FORMAT_TRANSIT,
};
