//! structures and functions common to both analytics and ingestion

#![allow(missing_docs)]

#[cfg(feature = "server")]
pub mod blob_storage;
pub mod block_wire_format;
pub mod compression;
#[cfg(feature = "server")]
pub mod property;
pub mod serde_byte_buf;
pub mod stream_info;
pub mod types;
pub mod wire_format;
