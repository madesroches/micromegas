//! `streams.format` literals — the keys used for per-block processor dispatch.
//!
//! Both `BlockPartitionSpec::block_processors` and the JIT
//! `write_partition_from_blocks` map are keyed by these strings; views register
//! one entry per format they understand. Keep these in sync with
//! `micromegas-otel-ingestion`'s constants and the writer-side `format` column
//! values inserted by `WebIngestionService::register_otel_stream`.

/// Native transit wire format (with a CBOR envelope around the transit-encoded
/// objects). Default for streams populated by `micromegas-tracing` producers.
pub const FORMAT_TRANSIT: &str = "micromegas-transit";

/// One `ResourceLogs` proto per block payload.
pub const FORMAT_OTLP_LOGS: &str = "otlp/v1/logs";

/// One `ResourceMetrics` proto per block payload.
pub const FORMAT_OTLP_METRICS: &str = "otlp/v1/metrics";

/// One `ResourceSpans` proto per block payload.
pub const FORMAT_OTLP_TRACES: &str = "otlp/v1/traces";
