//! High-performance tracing for logs, metrics, and spans.
//!
//! This crate provides low-overhead instrumentation (~20ns per event) for high-performance
//! applications. Originally designed for video game engines, it focuses on predictable
//! performance while providing comprehensive observability.
//!
//! # Quick Start - Instrumenting Functions
//!
//! The easiest way to instrument your code is with the [`#[span_fn]`](prelude::span_fn) attribute macro:
//!
//! ```rust,ignore
//! use micromegas_tracing::prelude::*;
//!
//! #[span_fn]
//! async fn fetch_user(id: u64) -> User {
//!     // Automatically tracks execution time, even across .await points
//!     database.get_user(id).await
//! }
//!
//! #[span_fn]
//! fn compute_hash(data: &[u8]) -> Hash {
//!     // Works for sync functions too
//!     hasher.hash(data)
//! }
//! ```
//!
//! [`#[span_fn]`](prelude::span_fn) is the primary tool for instrumenting async code. It correctly tracks
//! wall-clock time across await points by wrapping the future in an [`InstrumentedFuture`](prelude::InstrumentedFuture).
//!
//! # Logging
//!
//! ```rust,ignore
//! use micromegas_tracing::prelude::*;
//!
//! info!("User logged in");
//! warn!("Connection timeout");
//! error!("Failed to process request");
//! debug!("Debug info: {value}");
//! trace!("Detailed trace");
//! ```
//!
//! # Metrics
//!
//! ```rust,ignore
//! use micromegas_tracing::prelude::*;
//!
//! imetric!("requests_total", "count", 1);
//! fmetric!("response_time", "ms", elapsed_ms);
//! ```
//!
//! # Manual Span Scopes
//!
//! For fine-grained control within a function, use [`span_scope!`]:
//!
//! ```rust,ignore
//! use micromegas_tracing::prelude::*;
//!
//! fn process_batch(items: &[Item]) {
//!     span_scope!("process_batch");
//!
//!     for item in items {
//!         span_scope!("process_item");
//!         // Process each item...
//!     }
//! }
//! ```
//!
//! # Initialization
//!
//! Libraries should not initialize tracing - that's the application's responsibility.
//! Applications typically use `#[micromegas_main]` from the `micromegas` crate, or
//! manually set up guards:
//!
//! ```
//! use micromegas_tracing::{guards, event};
//!
//! // Application initialization (libraries should NOT do this)
//! let _tracing_guard = guards::TracingSystemGuard::new(
//!     8 * 1024 * 1024,  // log buffer size
//!     1024 * 1024,      // metrics buffer size
//!     16 * 1024 * 1024, // spans buffer size
//!     std::sync::Arc::new(event::NullEventSink {}),
//!     std::collections::HashMap::new(),
//!     true, // Enable CPU tracing
//! );
//! let _thread_guard = guards::TracingThreadGuard::new();
//! ```
//!
//! # Architecture
//!
//! Unlike other tracing crates, this library collects events into a stream rather than
//! providing per-event hooks. Events are serialized to a binary format using `transit`,
//! enabling efficient in-process buffering and network transmission.
//!

// crate-specific lint exceptions:
#![allow(
    unsafe_code,
    missing_docs,
    clippy::missing_errors_doc,
    clippy::inline_always
)]

pub mod dispatch;
pub mod errors;
pub mod event;
pub mod flush_monitor;
pub mod guards;
pub mod levels;
pub mod logs;
pub mod metrics;
pub mod panic_hook;
pub mod parsing;
pub mod process_info;
pub mod property_set;
#[cfg(feature = "tokio")]
pub mod runtime;
pub mod spans;
pub mod static_string_ref;
pub mod string_id;
pub mod test_utils;
pub mod time;

#[macro_use]
extern crate lazy_static;

#[macro_use]
mod macros;
pub mod intern_string;

/// Commonly used items for convenient importing - includes macros, types, and traits
pub mod prelude {
    pub use crate::levels::*;
    pub use crate::process_info::*;
    #[cfg(feature = "tokio")]
    pub use crate::runtime::TracingRuntimeExt;
    #[cfg(feature = "tokio")]
    pub use crate::spans::spawn_with_context;
    pub use crate::spans::{
        InstrumentFuture, InstrumentedFuture, InstrumentedNamedFuture, SpanScope, current_span_id,
    };
    pub use crate::time::*;
    pub use crate::{
        debug, error, fatal, fmetric, imetric, info, instrument_named, log, log_enabled,
        span_async_named, span_scope, span_scope_named, static_span_desc, static_span_location,
        trace, warn,
    };
    pub use micromegas_tracing_proc_macros::*;
}
