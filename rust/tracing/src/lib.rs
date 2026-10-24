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
//! A name or unit built at run time goes through [`imetric_dyn!`] and [`fmetric_dyn!`], which
//! copy it into the current block instead of requiring a `'static` string:
//!
//! ```rust,ignore
//! imetric_dyn!(format!("queue.{queue}.depth"), "count", depth);
//! ```
//!
//! # Dynamic properties
//!
//! Properties are `key = value` pairs, written before the format string in logs and before the
//! value in metrics. Keys written in the macros are identifiers or string literals and are always
//! `'static`. A literal value is interned once per callsite and costs one atomic load afterwards
//! (unless a `..set` is spread: then the literals before it are copied into the block like dynamic
//! values, while the spread set's own values are not). Any other value (a correlation, request or
//! session id) is attached to the individual log entry or measure and copied into the current
//! block, once per distinct value, and freed with it, so nothing is retained per distinct value:
//!
//! ```rust,ignore
//! use micromegas_tracing::prelude::*;
//!
//! let cid = request.correlation_id(); // String
//! info!(correlation_id = cid, region = "eu", rows = %n, "fetched {rows} rows");
//! imetric!("rows_fetched", "count", correlation_id = cid, region = "eu", n);
//! fmetric!("fetch_ms", "ms", "http.method" = "GET", elapsed_ms);
//! ```
//!
//! A bare value must implement [`PropertyValue`](property_value::PropertyValue) (strings do and
//! are borrowed without a copy). `%value` formats with `Display` and `?value` with `Debug`, at
//! the cost of one `String` allocation; those and bare values are only evaluated when the log
//! entry is enabled. A precomputed `&'static PropertySet` is spread as the last entry with
//! `..set`, and the pairs before it override its entries:
//!
//! ```rust,ignore
//! imetric!("rows_fetched", "count", correlation_id = cid, ..SET, n);
//! ```
//!
//! On the query side the values are ordinary properties:
//! `property_get(properties, 'correlation_id')`.
//!
//! In analytics some property names override the row's columns: `level` and `target` for logs,
//! `target`, `name` and `unit` for metrics. The macros reject those names as keys (`target`
//! belongs in `target: ..` before the other arguments); an override arrives through a `..set`
//! spread, a runtime [`DynProperty`](property_set::DynProperty) slice, or the
//! [`imetric_dyn!`]/[`fmetric_dyn!`] macros, which set `name` and `unit`. A dynamic `level` must
//! be a valid level name or the block fails to parse. Interop logs (the `tracing` layer) keep
//! their level and target unchanged.
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
//! # Span properties
//!
//! A span can carry properties on its end event, so that it reports its results (row counts,
//! cache hit or miss, an error status) as well as the context the caller attaches. Put per-call
//! context there rather than in a runtime name, which keeps span names low-cardinality. With
//! `name =`, the scope macros return the guard; the property list is the one the log macros take,
//! and `set_property` adds values later:
//!
//! ```rust,ignore
//! use micromegas_tracing::prelude::*;
//!
//! let mut span = span_scope!(name = "fetch_rows", region = "eu", request = %req_id);
//! let rows = fetch()?; // an early return still records region and request
//! span.set_property("rows", rows.len());
//! span.set_property("cache", if hit { "hit" } else { "miss" });
//! ```
//!
//! An async span takes a closure that receives the output of the future:
//!
//! ```rust,ignore
//! let rows = instrument_named!(fetch(id), "fetch", |out, span| {
//!     if let Ok(rows) = out {
//!         span.set_property("rows", rows.len());
//!     }
//! })
//! .await;
//! ```
//!
//! Gotchas:
//!
//! - `let _ = span_scope!(name = ..)` drops the guard at once, like any `let _`. Bind a name.
//! - A span that never ends, or that is cut at the edge of a query range, has no properties.
//! - Deploy analytics before instrumented code that records span properties.
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

// dispatch: use wasm version on wasm32
#[cfg(not(target_arch = "wasm32"))]
pub mod dispatch;
#[cfg(target_arch = "wasm32")]
#[path = "dispatch_wasm.rs"]
pub mod dispatch;

pub mod errors;
pub mod event;
pub mod guards;
#[cfg(not(target_arch = "wasm32"))]
pub mod images;
pub mod levels;
pub mod logs;
pub mod metrics;
pub mod panic_hook;
pub mod process_info;
pub mod property_value;
pub mod source_path;
pub mod spans;
pub mod time;

// Native-only: full on native, stubs in lib.rs for wasm32
#[cfg(not(target_arch = "wasm32"))]
pub mod dyn_deps;
#[cfg(not(target_arch = "wasm32"))]
pub mod property_set;
#[cfg(not(target_arch = "wasm32"))]
pub mod static_set_cache;

#[cfg(target_arch = "wasm32")]
pub mod static_set_cache {
    use crate::property_set::PropertySet;

    /// Only the empty set exists on wasm32.
    pub struct StaticSetCache<const M: usize>;

    impl<const M: usize> StaticSetCache<M> {
        pub const fn new() -> Self {
            Self
        }

        pub fn get(
            &self,
            _base: &'static PropertySet,
            _pairs: [(&'static str, &'static str); M],
        ) -> &'static PropertySet {
            PropertySet::empty()
        }
    }

    impl<const M: usize> Default for StaticSetCache<M> {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub mod property_set {
    /// Stub for EventSink trait compatibility — never constructed on wasm32
    pub struct Property;
    /// Stub for dispatch function signatures — only the empty set exists on wasm32
    pub struct PropertySet;

    impl PropertySet {
        pub fn empty() -> &'static Self {
            static EMPTY: PropertySet = PropertySet;
            &EMPTY
        }
    }

    /// Only the empty set exists on wasm32.
    pub fn callsite_set(
        _cell: &'static std::sync::OnceLock<&'static PropertySet>,
        _pairs: &'static [(&'static str, &'static str)],
    ) -> &'static PropertySet {
        PropertySet::empty()
    }

    pub fn static_value(v: &'static str) -> &'static str {
        v
    }

    /// A property whose value is not `'static`. The name is `'static` (`new`) or block-scoped
    /// (`with_dyn_name`).
    #[derive(Debug, Clone, Copy)]
    pub struct DynProperty<'a> {
        name: &'a str,
        value: &'a str,
    }

    impl<'a> DynProperty<'a> {
        pub const fn new(name: &'static str, value: &'a str) -> Self {
            Self { name, value }
        }

        pub const fn with_dyn_name(name: &'a str, value: &'a str) -> Self {
            Self { name, value }
        }

        pub fn name(&self) -> &'a str {
            self.name
        }

        pub fn value(&self) -> &'a str {
            self.value
        }
    }
}

// Native-only modules (entirely transit-dependent)
#[cfg(not(target_arch = "wasm32"))]
pub use dispatch::send_image;
#[cfg(not(target_arch = "wasm32"))]
pub mod flush_monitor;
#[cfg(not(target_arch = "wasm32"))]
pub mod intern_string;
#[cfg(not(target_arch = "wasm32"))]
pub mod parsing;
#[cfg(not(target_arch = "wasm32"))]
pub mod static_string_ref;
#[cfg(not(target_arch = "wasm32"))]
pub mod string_id;
#[cfg(not(target_arch = "wasm32"))]
pub mod test_utils;

#[cfg(feature = "tokio")]
pub mod runtime;

#[macro_use]
mod macros;

#[doc(hidden)]
pub use micromegas_tracing_proc_macros::__log_impl;

/// Commonly used items for convenient importing - includes macros, types, and traits
pub mod prelude {
    pub use crate::levels::*;
    pub use crate::process_info::*;
    pub use crate::property_set::DynProperty;
    #[cfg(feature = "tokio")]
    pub use crate::runtime::TracingRuntimeExt;
    #[cfg(feature = "tokio")]
    pub use crate::spans::spawn_with_context;
    pub use crate::spans::{
        InstrumentFuture, InstrumentedFuture, InstrumentedNamedFuture, SpanContextFuture,
        current_span_id,
    };
    pub use crate::time::*;
    pub use crate::{
        debug, error, fatal, fmetric, fmetric_dyn, imetric, imetric_dyn, info, instrument_named,
        instrument_named_dyn, log, log_enabled, span_async_named, span_async_named_dyn, span_scope,
        span_scope_named, span_scope_named_dyn, static_span_desc, static_span_location, trace,
        warn,
    };
    pub use micromegas_tracing_proc_macros::*;
}
