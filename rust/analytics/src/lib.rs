//! analytics : provides read access to the telemetry data lake

// crate-specific lint exceptions:
#![allow(clippy::missing_errors_doc)]

pub mod analytics_service;
pub mod arrow_utils;
pub mod call_tree;
pub mod delete;
pub mod lakehouse;
pub mod log_entries_table;
pub mod log_entry;
pub mod measure;
pub mod metadata;
pub mod metrics_table;
pub mod payload;
pub mod query_log_entries;
pub mod query_metrics;
pub mod query_spans;
pub mod query_thread_events;
pub mod scope;
pub mod span_table;
pub mod sql_arrow_bridge;
pub mod thread_block_processor;
pub mod thread_events_table;
pub mod time;
