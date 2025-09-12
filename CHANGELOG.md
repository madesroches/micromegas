# Changelog

This file documents the historical progress of the Micromegas project. For current focus, please see the main [README.md](./README.md).

## [Unreleased]
 * **Data Processing & Performance:**
   * Dictionary encoding support with properties UDFs (`properties_to_dict`, `properties_to_array`, `properties_length`) (#506, #507, #508)
   * Fixed parquet metadata race conditions with separation strategy (#502, #504)
   * Optimized lakehouse partition queries by removing unnecessary file_metadata fetches (#499)
   * Scalability improvements for high-volume environments (#497, #498)
 * **Monitoring & Analytics:**
   * Added `log_stats` SQL aggregation view for log analysis by severity and service (#495, #505)
   * Refactored lakehouse views to use `generate_process_jit_partitions` (#493)
 * Working on aggregate log views for enhanced stability monitoring
 * Planning improvements to log aggregation and health reporting systems

## September 2025
 * Released [version 0.12.0](https://crates.io/crates/micromegas)
 * **Major Features:**
   * Comprehensive async span tracing with `micromegas_main` proc macro (#451)
   * Named async span event tracking with improved API ergonomics (#475)
   * Async span depth tracking for performance analysis (#474)
   * Async trait tracing support in `span_fn` macro (#469)
   * Perfetto async spans support with trace generation (#485)
   * HTTP gateway for easier interoperability (#433, #435, #436)
   * JSONB support for flexible data structures (#409)
 * **Infrastructure & Performance:**
   * Consolidate Perfetto trace generation to use SQL-powered implementation (#489)
   * Query latency tracking and async span instrumentation optimization (#468)
   * Replace custom interning logic with `internment` crate (#430)
   * Optimize view_instance metadata requests (#450)
   * Convert all unit tests to in-memory recording (#472)
 * **Documentation & Developer Experience:**
   * Complete Python API documentation with comprehensive docstrings (#491)
   * Complete SQL functions documentation with all missing UDFs/UDAFs/UDTFs (#470)
   * Visual architecture diagrams in documentation (#462)
   * Unreal instrumentation documentation (#492)
   * Automated documentation publishing workflow (#444)
 * **Security & Dependencies:**
   * Fix CVE-2025-58160: Update tracing-subscriber to 0.3.20 (#490)
   * Update DataFusion, tokio and other dependencies (#429, #476)
   * Rust edition 2024 upgrade with unsafe operations fixes (#408)
 * **Web UI & Export:**
   * Export Perfetto traces from web UI (#482)
   * Analytics web app build fixes and documentation updates (#483)
 * **Cloud & Deployment:**
   * Docker deployment scripts (#422)
   * Amazon Linux setup script (#423)
   * Cloud environment configuration support (#426)
   * Configurable PostgreSQL port via MICROMEGAS_DB_PORT (#425)

## July 2025
 * Released [version 0.11.0](https://crates.io/crates/micromegas)
 * Working on http gateway for easier interoperability
 * Add export mechanism to view materialization to send data out as it is ingested

## June 2025
 * Released [version 0.10.0](https://crates.io/crates/micromegas)
 * Process properties in measures and log_entries
 * Better histogram support
 * Processes and streams views now contain all processes/streams updated in the requested time range - based on SqlBatchView.

## May 2025
 * Released [version 0.8.0](https://crates.io/crates/micromegas) and [version 0.9.0](https://crates.io/crates/micromegas)
 * Frame budget reporting
 * Histogram support with quantile estimation
 * Run seconds & minutes tasks in parallel in daemon
 * GetPayload user defined function
 * Add bulk ingestion API for replication

## April 2025
 * Released [version 0.7.0](https://crates.io/crates/micromegas)
 * Perfetto trace server
 * DataFusion memory budget
 * Memory optimizations
 * Fixed interning of property sets
 * More flexible trace macros

## March 2025
 * Released [version 0.5.0](https://crates.io/crates/micromegas)
 * Better perfetto support
 * New rust FlightSQL client
 * Unreal crash reporting

## February 2025
 * Released [version 0.4.0](https://crates.io/crates/micromegas)
 * Incremental data reduction using sql-defined views
 * System monitor thread
 * Added support for ARM (& macos)
 * Deleted analytics-srv and the custom http python client to connect to it
 
## January 2025
 * Released [version 0.3.0](https://crates.io/crates/micromegas)
 * New FlightSQL python API
   * Ready to replace analytics-srv with flight-sql-srv

## December 2024
 * [Grafana plugin](https://github.com/madesroches/grafana-micromegas-datasource/)
 * Released [version 0.2.3](https://crates.io/crates/micromegas)
 * Properties on measures & log entries available in SQL queries

## November 2024
Released [version 0.2.1](https://crates.io/crates/micromegas)

 * FlightSQL support
 * Measures and log entries can now be tagged with properties
   * Not yet available in SQL queries

## October 2024
Released [version 0.2.0](https://crates.io/crates/micromegas)

 * Unified the query interface
   * Using `view_instance` table function to materialize just-in-time process-specific views from within SQL
 * Updated python doc to reflect the new API: https://pypi.org/project/micromegas/

## September 2024
Released [version 0.1.9](https://crates.io/crates/micromegas)

 * Updating global views every second
 * Caching metadata (processes, streams & blocks) in the lakehouse & allow sql queries on them

## August 2024
Released [version 0.1.7](https://crates.io/crates/micromegas)

 * New global materialized views for logs & metrics of all processes
 * New daemon service to keep the views updated as data is ingested
 * New analytics API based on SQL powered by Apache DataFusion

## July 2024
Released [version 0.1.5](https://crates.io/crates/micromegas)

Unreal
 * Better reliability, retrying failed http requests
 * Spike detection

Maintenance
 * Delete old blocks, streams & processes using cron task

## June 2024
Released [version 0.1.4](https://crates.io/crates/micromegas)

Good enough for dogfooding :)

Unreal
 * Metrics publisher
 * FName scopes

Analytics
 * Metric queries
 * Convert cpu traces in perfetto format

## May 2024
Released [version 0.1.3](https://crates.io/crates/micromegas)

Better unreal engine instrumentation
  * new protocol
  * http request callbacks no longer binded to the main thread
  * custom authentication of requests

Analytics
  * query process metadata
  * query spans of a thread

## April 2024
Telemetry ingestion from rust & unreal are working :) 

Released [version 0.1.1](https://crates.io/crates/micromegas)

Not actually useful yet, I need to bring back the analytics service to a working state.

## January 2024
Starting anew. I'm extracting the tracing/telemetry/analytics code from https://github.com/legion-labs/legion to jumpstart the new project. If you are interested in collaborating, please reach out.