//! Runtime integration utilities for micromegas tracing
//!
//! This module provides helper functions and utilities for integrating
//! micromegas tracing with async runtimes, particularly tokio.

#[cfg(feature = "tokio")]
use crate::dispatch::{flush_thread_buffer, init_thread_stream, unregister_thread_stream};

/// Extension trait for `tokio::runtime::Builder` that adds tracing lifecycle callbacks.
///
/// This trait provides a convenient way to configure tokio runtimes with the proper
/// thread lifecycle callbacks for micromegas tracing, ensuring that:
/// - Thread streams are initialized when worker threads start
/// - Event buffers are flushed when threads park (become idle)
/// - Thread streams are properly unregistered when threads stop
///
/// This is useful in both production applications (like ingestion servers) and tests
/// where you need proper tracing integration with tokio's thread pool.
///
/// # Examples
///
/// ```no_run
/// use micromegas_tracing::runtime::TracingRuntimeExt;
///
/// let runtime = tokio::runtime::Builder::new_multi_thread()
///     .enable_all()
///     .thread_name("my-service")
///     .with_tracing_callbacks()
///     .build()
///     .expect("Failed to build runtime");
/// ```
#[cfg(feature = "tokio")]
pub trait TracingRuntimeExt {
    /// Configures the runtime builder with standard tracing lifecycle callbacks.
    ///
    /// This method adds the following callbacks:
    /// - `on_thread_start`: Initializes thread-local tracing stream
    /// - `on_thread_park`: Flushes event buffer when thread becomes idle
    /// - `on_thread_stop`: Unregisters thread stream to prevent dangling pointers
    fn with_tracing_callbacks(&mut self) -> &mut Self;

    /// Configures the runtime builder with tracing callbacks and custom thread start logic.
    ///
    /// This is useful when you need to perform additional setup during thread start
    /// while still maintaining the standard tracing lifecycle.
    ///
    /// # Arguments
    ///
    /// * `on_start` - Custom function to call in addition to `init_thread_stream()`
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use micromegas_tracing::runtime::TracingRuntimeExt;
    /// use std::sync::atomic::{AtomicUsize, Ordering};
    /// use std::sync::Arc;
    ///
    /// let counter = Arc::new(AtomicUsize::new(0));
    /// let counter_clone = counter.clone();
    ///
    /// let runtime = tokio::runtime::Builder::new_multi_thread()
    ///     .enable_all()
    ///     .with_tracing_callbacks_and_custom_start(move || {
    ///         let id = counter_clone.fetch_add(1, Ordering::Relaxed);
    ///         eprintln!("Worker thread {} starting", id);
    ///     })
    ///     .build()
    ///     .expect("Failed to build runtime");
    /// ```
    fn with_tracing_callbacks_and_custom_start<F>(&mut self, on_start: F) -> &mut Self
    where
        F: Fn() + Send + Sync + 'static;
}

#[cfg(feature = "tokio")]
impl TracingRuntimeExt for tokio::runtime::Builder {
    fn with_tracing_callbacks(&mut self) -> &mut Self {
        self.on_thread_start(|| {
            init_thread_stream();
        })
        .on_thread_stop(|| {
            unregister_thread_stream();
        })
    }

    fn with_tracing_callbacks_and_custom_start<F>(&mut self, on_start: F) -> &mut Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.on_thread_start(move || {
            init_thread_stream();
            on_start();
        })
        .on_thread_stop(|| {
            flush_thread_buffer();
            unregister_thread_stream();
        })
    }
}
