//! Library to write Perfetto traces, part of Micromegas.
#![allow(missing_docs, clippy::missing_errors_doc)]

/// Protobufs (generated code)
#[allow(
    clippy::doc_lazy_continuation,
    clippy::len_without_is_empty,
    clippy::large_enum_variant,
    clippy::doc_overindented_list_items
)]
#[cfg(not(feature = "protogen"))]
pub mod protos {
    include!("perfetto.protos.rs");
}

/// Async writer trait
#[cfg(not(feature = "protogen"))]
pub mod async_writer;

/// Utility functions
#[cfg(not(feature = "protogen"))]
pub mod utils;

/// Streaming Trace Writer
#[cfg(not(feature = "protogen"))]
pub mod streaming_writer;

/// Chunk sender for streaming traces
#[cfg(not(feature = "protogen"))]
pub mod chunk_sender;

#[cfg(not(feature = "protogen"))]
pub use streaming_writer::PerfettoWriter;

#[cfg(not(feature = "protogen"))]
pub use async_writer::AsyncWriter;

#[cfg(not(feature = "protogen"))]
pub use chunk_sender::ChunkSender;
