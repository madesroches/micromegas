//! OTLP/HTTP ingestion adapter for the micromegas data lake.
//!
//! This crate accepts OTLP `Export*ServiceRequest` proto messages and writes them as
//! micromegas blocks. Translation from proto to parquet rows happens at the analytics
//! layer; this crate's job is just identity synthesis (resource → process_id /
//! stream_id), per-resource block splitting, and idempotent SQL inserts.

#![allow(missing_docs, clippy::missing_errors_doc)]

pub mod block;
pub mod error;
pub mod handler;
pub mod identity;
pub mod proto;

pub use error::{OtelError, Signal};

/// `tsc_frequency` value recorded on processes synthesized from OTLP resources.
/// OTLP timestamps are absolute nanoseconds, so 1 tick = 1 ns.
pub const OTLP_TICKS_PER_SECOND: i64 = 1_000_000_000;

/// Stream tag for OTel logs (shared with native log producers — `log_entries` view loads both).
pub const TAG_LOGS: &str = "log";
/// Stream tag for OTel metrics (shared with native metrics producers).
pub const TAG_METRICS: &str = "metrics";
/// Stream tag for OTel traces (new — native async spans use the `cpu` tag).
pub const TAG_TRACES: &str = "trace";

// Format constants live in `micromegas-ingestion` so writer-side (this crate) and
// reader-side (`micromegas-analytics`) read from a single source of truth.
pub use micromegas_ingestion::web_ingestion_service::{
    FORMAT_OTLP_LOGS, FORMAT_OTLP_METRICS, FORMAT_OTLP_TRACES,
};
