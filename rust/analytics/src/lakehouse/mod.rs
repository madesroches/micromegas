/// Record batches + schema
pub mod answer;
/// Implementation of `BlockProcessor` for async events
pub mod async_events_block_processor;
/// Materializable view of async span events accessible through datafusion
pub mod async_events_view;
/// Write parquet in object store
pub mod async_parquet_writer;
/// BatchPartitionMerger merges multiple partitions by splitting the work in batches to use less memory.
/// The batches are based on event times.
pub mod batch_partition_merger;
/// Materialize views on a schedule based on the time data was received from the ingestion service
pub mod batch_update;
/// Specification for a view partition backed by a set of telemetry blocks which can be processed out of order
pub mod block_partition_spec;
/// Replicated view of the `blocks` table of the postgresql metadata database.
pub mod blocks_view;
/// Catalog utilities for discovering and managing view schemas
pub mod catalog;
pub mod dataframe_time_bounds;
/// Export mechanism that doubles as audit trail
pub mod export_log_view;
/// Fetch payload from the object store using SQL
pub mod get_payload_function;
/// Management of process-specific partitions built on demand
pub mod jit_partitions;
/// Read access to the list of lakehouse partitions
pub mod list_partitions_table_function;
/// Read access to view sets with their schema information
pub mod list_view_sets_table_function;
/// Implementation of `BlockProcessor` for log entries
pub mod log_block_processor;
/// SQL-based view for log statistics aggregated by process, minute, level, and target
pub mod log_stats_view;
/// Materializable view of log entries accessible through datafusion
pub mod log_view;
/// Exposes materialize_partitions as a table function
pub mod materialize_partitions_table_function;
/// TableProvider implementation for the lakehouse
pub mod materialized_view;
/// Merge consecutive parquet partitions into a single file
pub mod merge;
/// Global LRU cache for partition metadata
pub mod metadata_cache;
/// Compatibility layer for parsing legacy Arrow 56.0 metadata and upgrading to Arrow 57.0
pub mod metadata_compat;
/// Specification for a view partition backed by a table in the postgresql metadata database.
pub mod metadata_partition_spec;
/// Implementation of `BlockProcessor` for measures
pub mod metrics_block_processor;
/// Materializable view of measures accessible through datafusion
pub mod metrics_view;
/// Maintenance of the postgresql tables and indices use to track the parquet files used to implement the views
pub mod migration;
/// Write & delete sections of views
pub mod partition;
/// In-memory copy of a subnet of the list of the partitions in the db
pub mod partition_cache;
/// Operations on the dedicated partition_metadata table
pub mod partition_metadata;
/// Describes the event blocks backing a partition
pub mod partition_source_data;
/// ExecutionPlan based on a set of parquet files
pub mod partitioned_execution_plan;
/// TableProvider based on a set of parquet files
pub mod partitioned_table_provider;
/// ExecutionPlan for generating Perfetto trace chunks
pub mod perfetto_trace_execution_plan;
/// Table function for generating Perfetto trace chunks
pub mod perfetto_trace_table_function;
/// Replicated view of the `processes` table of the postgresql metadata database.
pub mod processes_view;
/// property_get function support from SQL
/// Datafusion integration
pub mod query;
/// Wrapper around ParquetObjectreader to provide ParquetMetaData without hitting the ObjectStore
pub mod reader_factory;
/// Scalar UDF to retire a single partition by file path
pub mod retire_partition_by_file_udf;
/// Scalar UDF to retire a single partition by metadata
pub mod retire_partition_by_metadata_udf;
/// Exposes retire_partitions as a table function
pub mod retire_partitions_table_function;
/// Runtime resources
pub mod runtime;
/// SessionConfigurator trait for custom session context configuration
pub mod session_configurator;
/// Sql-defined view updated in batch
pub mod sql_batch_view;
/// Specification for a view partition backed by a SQL query on the lakehouse.
pub mod sql_partition_spec;
/// Replicated view of the `streams` table of the postgresql metadata database.
pub mod streams_view;
/// Rewrite table scans to take the query range into account
pub mod table_scan_rewrite;
/// Tracking of expired partitions
pub mod temp;
/// Jit view of the call tree built from the thread events of a single stream
pub mod thread_spans_view;
/// Basic interface for a set of rows queryable and materializable
pub mod view;
pub mod view_factory;
/// Table function to query process-specific views
pub mod view_instance_table_function;
/// Add or remove view partitions
pub mod write_partition;
