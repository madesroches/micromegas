
# Micromegas - Scalable Observability

[![Crates.io][crates-badge]][crates-url]
[![Apache licensed][license-badge]][license-url]
[![Build Status][actions-badge]][actions-url]

[rust API documentation](https://docs.rs/micromegas/latest/micromegas/) 

[python API](https://pypi.org/project/micromegas/)

[grafana plugin](https://github.com/madesroches/grafana-micromegas-datasource/)

[design presentation](https://madesroches.github.io/micromegas/doc/design-presentation/design.html) 

[unreal observability](https://madesroches.github.io/micromegas/doc/unreal-observability/unreal-observability.html)


[crates-badge]: https://img.shields.io/crates/v/micromegas.svg
[crates-url]: https://crates.io/crates/micromegas
[license-badge]: https://img.shields.io/badge/license-Apache%20v2-blue.svg
[license-url]: https://github.com/madesroches/micromegas/blob/main/LICENSE
[actions-badge]: https://github.com/madesroches/micromegas/actions/workflows/rust.yml/badge.svg
[actions-url]: https://github.com/madesroches/micromegas/actions?query=branch%3Amain


## Objectives

 * Unified observability: logs, metrics and traces in the same database.
 
 * Spend less time reproducing problems

   * Collect enough data to understand how to correct the problems. 
 
   * Quantify the frequency and severity of the issues instead of debugging the first one you can reproduce.
 
 * Achieve better quality: monitor & catch problems before they get noticed by users.
 
## Design Strategies


### Low overhead instrumentation

20 ns / event in the calling thread, one additional thread for the preparation and upload to the server.

### High frequency of events

Up to 100000 events / second for a single instrumented process.

### Scalability of ingestion service

Scalable backend can accept data from millions of concurrent instrumented processes.

  * Data stored in S3
  * Metadata stored in [PostgreSQL](https://www.postgresql.org/)

### Tail sampling & ETL on demand

In order to keep costs down, most payloads will remain unprocessed until they expire.

### Query using SQL

 * Query engine powered by [DataFusion](https://arrow.apache.org/datafusion/)
 * Python and Rust clients built on [FlightSQL](https://arrow.apache.org/blog/2022/02/16/introducing-arrow-flight-sql/)
 * PostgreSQL-compatible gateway using [DataFusion-postgres](https://github.com/datafusion-contrib/datafusion-postgres) and [pgwire](https://github.com/sunng87/pgwire)


## Status

### July 2025
 * [pg-gateway-srv](https://github.com/madesroches/micromegas/tree/main/rust/pg-gateway-srv) for query federation

### June 2025
 * Released [version 0.10.0](https://crates.io/crates/micromegas)
 * Process properties in measures and log_entries
 * Better histogram support
 * Processes and streams views now contain all processes/streams updated in the requested time range - based on SqlBatchView.

### May 2025
 * Released [version 0.8.0](https://crates.io/crates/micromegas) and [version 0.9.0](https://crates.io/crates/micromegas)
 * Frame budget reporting
 * Histogram support with quantile estimation
 * Run seconds & minutes tasks in parallel in daemon
 * GetPayload user defined function
 * Add bulk ingestion API for replication

### April 2025
 * Released [version 0.7.0](https://crates.io/crates/micromegas)
 * Perfetto trace server
 * DataFusion memory budget
 * Memory optimizations
 * Fixed interning of property sets
 * More flexible trace macros

### March 2025
 * Released [version 0.5.0](https://crates.io/crates/micromegas)
 * Better perfetto support
 * New rust FlightSQL client
 * Unreal crash reporting

### February 2025

 * Released [version 0.4.0](https://crates.io/crates/micromegas)
 * Incremental data reduction using sql-defined views
 * System monitor thread
 * Added support for ARM (& macos)
 * Deleted analytics-srv and the custom http python client to connect to it
 
### January 2025

 * Released [version 0.3.0](https://crates.io/crates/micromegas)
 * New FlightSQL python API
   * Ready to replace analytics-srv with flight-sql-srv

### Decembre 2024

 * [Grafana plugin](https://github.com/madesroches/grafana-micromegas-datasource/)
 * Released [version 0.2.3](https://crates.io/crates/micromegas)
 * Properties on measures & log entries available in SQL queries

### Novembre 2024
Released [version 0.2.1](https://crates.io/crates/micromegas)

 * FlightSQL support
 * Measures and log entries can now be tagged with properties
   * Not yet available in SQL queries

### October 2024
Released [version 0.2.0](https://crates.io/crates/micromegas)

 * Unified the query interface
   * Using [`view_instance`](https://docs.rs/micromegas/latest/micromegas/analytics/lakehouse/view_instance_table_function/struct.ViewInstanceTableFunction.html) table function to materialize just-in-time process-specific views from within SQL
 * Updated python doc to reflect the new API: https://pypi.org/project/micromegas/

### Septembre 2024
Released [version 0.1.9](https://crates.io/crates/micromegas)

 * Updating global views every second
 * Caching metadata (processes, streams & blocks) in the lakehouse & allow sql queries on them

### August 2024
Released [version 0.1.7](https://crates.io/crates/micromegas)

 * New global materialized views for logs & metrics of all processes
 * New daemon service to keep the views updated as data is ingested
 * New analytics API based on SQL powered by Apache DataFusion

### July 2024
Released [version 0.1.5](https://crates.io/crates/micromegas)

Unreal
 * Better reliability, retrying failed http requests
 * Spike detection

Maintenance
 * Delete old blocks, streams & processes using cron task

### June 2024
Released [version 0.1.4](https://crates.io/crates/micromegas)

Good enough for dogfooding :)

Unreal
 * Metrics publisher
 * FName scopes

Analytics
 * Metric queries
 * Convert cpu traces in perfetto format

### May 2024
Released [version 0.1.3](https://crates.io/crates/micromegas)

Better unreal engine instrumentation
  * new protocol
  * http request callbacks no longer binded to the main thread
  * custom authentication of requests

Analytics
  * query process metadata
  * query spans of a thread

### April 2024
Telemetry ingestion from rust & unreal are working :) 

Released [version 0.1.1](https://crates.io/crates/micromegas)

Not actually useful yet, I need to bring back the analytics service to a working state.

### January 2024
Starting anew. I'm extracting the tracing/telemetry/analytics code from https://github.com/legion-labs/legion to jumpstart the new project. If you are interested in collaborating, please reach out.
