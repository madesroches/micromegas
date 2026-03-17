/// Write log entries as a SendableRecordBatchStream
pub mod async_log_stream;
/// Helper to create CSV table providers
pub mod csv_table_provider;
/// Utilities to help deal with df expressions
pub mod expressions;
/// Helper to create JSON table providers
pub mod json_table_provider;
/// Stream a function's log as a table
pub mod log_stream_table_provider;
/// Convert a filtering expression to a physical predicate
pub mod predicate;
/// Unified string column accessor for Arrow arrays
pub mod string_column_accessor;
/// Execution plan interface for an async task
pub mod task_log_exec_plan;
/// Access to a RecordBatch's columns
pub mod typed_column;

// Re-export from micromegas-datafusion-extensions
pub use micromegas_datafusion_extensions::binary_column_accessor;
pub use micromegas_datafusion_extensions::histogram;
pub use micromegas_datafusion_extensions::jsonb;
