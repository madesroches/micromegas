![rust build](https://github.com/madesroches/micromegas/actions/workflows/rust.yml/badge.svg)

# micromegas
Scalable Observability

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
Starting anew. I'm extracting the tracing/telemetry/analytics code from https://github.com/legion-labs/legion to jumpstart the new project. If you are interested in collaborating, please reach out.
