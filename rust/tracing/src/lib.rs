//! Tracing crate
//!
//! Provides logging, metrics, memory and performance profiling
//!
//! Have the lowest impact on the critical path of execution while providing great
//! visibility, `tracing` focusses on providing predictable performance for high
//! performance applications. It's primary client is Legion Engine, which runs a
//! distributed, highly compute demanding workloads.
//!
//! Contrary to other tracing crates, tracing does not provide hooks for individual
//! events but rather a stream of events, internally it leverages transit
//! to serialize the events into a binary format. meant to be consumed later on in process
//! but can also be sent efficiently over the wire.
//!
//! # Examples
//! ```
//! use tracing::{
//!    span_scope, info, warn, error, debug, imetric, fmetric, guards, event,
//! };
//!
//! // Initialize tracing, here with a null event sink, see `lgn-telemetry-sink` crate for a proper implementation
//! // libraries don't need (and should not) setup any TracingSystemGuard
//! let _tracing_guard = guards::TracingSystemGuard::new(
//!     8 * 1024 * 1024,
//!     1024 * 1024,
//!     16 * 1024 * 1024,
//!     std::sync::Arc::new(event::NullEventSink {})
//! );
//! let _thread_guard = guards::TracingThreadGuard::new();
//!
//! // Create a span scope, this will complete when the scope is dropped, and provide the time spent in the scope
//! // Behind the scene this uses a thread local storage
//! // on an i9-11950H this takes around 40ns
//! span_scope!("main");
//!
//! // Logging
//! info!("Hello world");
//! warn!("Hello world");
//! error!("Hello world");
//! debug!("Hello world");
//!
//! // Metrics
//! imetric!("name", "unit", 0);
//! fmetric!("name", "unit", 0.0);
//! ```
//!

// crate-specific lint exceptions:
#![allow(unsafe_code, clippy::missing_errors_doc, clippy::inline_always)]

use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub process_id: String,
    pub exe: String,
    pub username: String,
    pub realname: String,
    pub computer: String,
    pub distro: String,
    pub cpu_brand: String,
    pub tsc_frequency: String,
    /// RFC 3339
    pub start_time: String,
    pub start_ticks: String,
    pub parent_process_id: String,
}

pub mod dispatch;
pub mod errors;
pub mod event;
pub mod guards;
pub mod logs;
pub mod metrics;
pub mod panic_hook;
pub mod spans;
pub mod string_id;

#[macro_use]
mod macros;
pub mod flush_monitor;
mod levels;
mod time;

pub mod prelude {
    pub use crate::levels::*;
    pub use crate::time::*;
    pub use crate::{
        async_span_scope, debug, error, fmetric, imetric, info, log, log_enabled, span_scope,
        trace, warn,
    };
    pub use tracing_proc_macros::*;
}

pub use prelude::*;
