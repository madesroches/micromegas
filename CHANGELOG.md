# Changelog

This file documents the historical progress of the Micromegas project. For current focus, please see the main [README.md](./README.md).

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