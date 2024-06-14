
# Micromegas - Scalable Observability

[![Crates.io][crates-badge]][crates-url]
[![Apache licensed][license-badge]][license-url]
[![Build Status][actions-badge]][actions-url]

[rust api documentation](https://docs.rs/micromegas/latest/micromegas/) 

[design presentation](https://madesroches.github.io/micromegas/doc/design.html) 


[crates-badge]: https://img.shields.io/crates/v/micromegas.svg
[crates-url]: https://crates.io/crates/micromegas
[license-badge]: https://img.shields.io/badge/license-Apache%20v2-blue.svg
[license-url]: https://github.com/madesroches/micromegas/blob/main/LICENSE
[actions-badge]: https://github.com/madesroches/micromegas/actions/workflows/rust.yml/badge.svg
[actions-url]: https://github.com/madesroches/micromegas/actions?query=branch%3Amain


## Objectives

### Low overhead instrumentation

20 ns / event in the calling thread, one additional thread for the preparation and upload to the server.

### High frequency of events

Up to 100000 events / second for a single instrumented process.

### Scalability of ingestion service

Scalable backend can accept data from millions of concurrent instrumented processes.

### Tail sampling & ETL on demand

In order to keep costs down, most payloads will remain unprocessed until they expire.

### Query using SQL

 * Analytics built on https://arrow.apache.org/datafusion/
 * Metadata stored in https://www.postgresql.org/

## Status

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
