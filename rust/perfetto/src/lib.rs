//! Library to write Perfetto traces, part of Micromegas.
#![allow(missing_docs, clippy::missing_errors_doc)]

// The library modules below are emptied out only while regenerating the
// protobuf bindings (the `regen_protos` cfg, set by `build.rs` when the
// `MICROMEGAS_REGEN_PROTOS` env var is present). This lets the
// `update-perfetto-protos` binary build and run even when the committed
// `perfetto.protos.rs` does not compile against the current `prost` runtime.
// In every normal build — including `--all-features` and `cargo doc` — the
// cfg is unset, so the full public API is always available.

/// Protobufs (generated code)
#[allow(
    clippy::doc_lazy_continuation,
    clippy::len_without_is_empty,
    clippy::large_enum_variant,
    clippy::doc_overindented_list_items
)]
#[cfg(not(regen_protos))]
pub mod protos {
    include!("perfetto.protos.rs");
}

/// Async writer trait
#[cfg(not(regen_protos))]
pub mod async_writer;

/// Utility functions
#[cfg(not(regen_protos))]
pub mod utils;

/// Streaming Trace Writer
#[cfg(not(regen_protos))]
pub mod streaming_writer;

/// Chunk sender for streaming traces
#[cfg(not(regen_protos))]
pub mod chunk_sender;
