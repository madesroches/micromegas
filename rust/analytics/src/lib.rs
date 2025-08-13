//! Micromegas analytics: makes the telemetry data lake accessible and useful.

// crate-specific lint exceptions:
#![allow(missing_docs, clippy::missing_errors_doc)]

/// Convert arrow array into Property instances
pub mod arrow_properties;
/// Misc arrow utilities
pub mod arrow_utils;
/// In-memory async events in arrow format
pub mod async_events_table;
/// Transforms thread events into call trees
pub mod call_tree;
/// Removal of old data
pub mod delete;
/// Datafusion extensions
pub mod dfext;
/// Module dedicated to the maintenance and query of materialized views
///
/// Unlike the telemetry data lake where it's fast & cheap to write but costly to read,
/// the lakehouse partitions are costly to write but allow for cheap & fast queries using datafusion.
///
/// Views based on a low frequency of events (< 1k events per second per process) are kept updated regularly.
/// Views based on a high frequency of events (up to 100k events per second per process) are metrialized on demand.
pub mod lakehouse;
/// In-memory log entries in arrow format
pub mod log_entries_table;
/// Parsing of log entries from telemetry payload
pub mod log_entry;
/// Parsing of metrics from telemetry payload
pub mod measure;
/// Access to the metadata stored in the relational database
pub mod metadata;
/// In-memory metrics in arrow format
pub mod metrics_table;
/// Access to the raw binary telemetry payload
pub mod payload;
/// Reference-counted set of properties in transit format
pub mod property_set;
pub mod record_batch_transformer;
/// bulk metadata & payload ingestion using Arrow
pub mod replication;
/// Streams response for long requests
pub mod response_writer;
/// Location in instrumented source code
pub mod scope;
/// In-memory call tree in arrow format
pub mod span_table;
/// Convert sqlx rows into arrow format
pub mod sql_arrow_bridge;
/// Parses thread event streams
pub mod thread_block_processor;
/// Conversion between ticks and more convenient date/time representations
pub mod time;
