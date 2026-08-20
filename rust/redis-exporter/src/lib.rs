//! Redis metrics exporter: samples one Redis server and emits `redis_*`
//! metrics through the micromegas telemetry sink.
pub mod cli;
pub mod info_parser;
pub mod sampler;
