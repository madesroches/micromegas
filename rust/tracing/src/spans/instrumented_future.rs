//! Manual async span instrumentation using InstrumentedFuture wrapper

use crate::dispatch::{
    on_begin_async_named_scope, on_begin_async_scope, on_end_async_named_scope, on_end_async_scope,
};
use crate::spans::{SpanLocation, SpanMetadata};
use pin_project::pin_project;
use std::cell::UnsafeCell;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

thread_local! {
    static ASYNC_CALL_STACK: UnsafeCell<Vec<u64>> = UnsafeCell ::new(vec![0]);
}

/// Returns the current span ID from the async call stack.
/// Returns 0 (root) if no span is active.
#[inline]
pub fn current_span_id() -> u64 {
    ASYNC_CALL_STACK.with(|stack_cell| {
        let stack = unsafe { &*stack_cell.get() };
        stack.last().copied().unwrap_or(0)
    })
}

/// A guard that establishes a span context for the duration of its lifetime.
/// Used to propagate span context across spawn boundaries.
///
/// # Example
/// ```ignore
/// let parent_span = current_span_id();
/// tokio::spawn(async move {
///     let _guard = SpanScope::new(parent_span);
///     // work here will see parent_span as its parent
/// });
/// ```
pub struct SpanScope {
    _private: (), // prevent direct construction
}

impl SpanScope {
    /// Creates a new span scope, pushing the given span ID onto the async call stack.
    #[inline]
    pub fn new(span_id: u64) -> Self {
        ASYNC_CALL_STACK.with(|stack_cell| {
            let stack = unsafe { &mut *stack_cell.get() };
            stack.push(span_id);
        });
        Self { _private: () }
    }
}

impl Drop for SpanScope {
    #[inline]
    fn drop(&mut self) {
        ASYNC_CALL_STACK.with(|stack_cell| {
            let stack = unsafe { &mut *stack_cell.get() };
            if stack.len() > 1 {
                stack.pop();
            }
        });
    }
}

/// Spawns a future on the tokio runtime while preserving the current span context.
///
/// This is a wrapper around `tokio::spawn` that captures the current span ID
/// before spawning and establishes it as the parent context in the spawned task.
/// This ensures that instrumented async functions called within the spawned task
/// will correctly report the spawning context as their parent.
///
/// # Example
/// ```ignore
/// use micromegas_tracing::prelude::*;
///
/// #[span_fn]
/// async fn parent_work() {
///     // Spans created in child_work will show parent_work as their parent
///     spawn_with_context(child_work()).await.unwrap();
/// }
///
/// #[span_fn]
/// async fn child_work() {
///     // This span's parent will be parent_work, not root
/// }
/// ```
#[cfg(feature = "tokio")]
pub fn spawn_with_context<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let parent_span = current_span_id();
    tokio::spawn(async move {
        let _scope = SpanScope::new(parent_span);
        future.await
    })
}

/// Trait for adding instrumentation to futures
pub trait InstrumentFuture: Future + Sized {
    /// Instrument this future with the given span metadata
    fn instrument(self, span_desc: &'static SpanMetadata) -> InstrumentedFuture<Self> {
        InstrumentedFuture::new(self, span_desc)
    }

    /// Internal method for named instrumentation - do not use directly.
    /// Use the `instrument_named!` macro for method-like syntax instead.
    #[doc(hidden)]
    fn __instrument_named_internal(
        self,
        span_location: &'static SpanLocation,
        name: &'static str,
    ) -> InstrumentedNamedFuture<Self> {
        InstrumentedNamedFuture::new(self, span_location, name)
    }
}

impl<F: Future> InstrumentFuture for F {}

/// A wrapper that instruments a future with async span tracing
#[pin_project]
pub struct InstrumentedFuture<F> {
    #[pin]
    future: F,
    desc: &'static SpanMetadata,
    span_id: Option<u64>,
    /// Parent span ID captured at future creation time
    parent: u64,
}

impl<F> InstrumentedFuture<F> {
    /// Create a new instrumented future
    pub fn new(future: F, desc: &'static SpanMetadata) -> Self {
        let parent = ASYNC_CALL_STACK.with(|stack_cell| {
            let stack = unsafe { &*stack_cell.get() };
            assert!(!stack.is_empty());
            stack[stack.len() - 1]
        });
        Self {
            future,
            desc,
            span_id: None,
            parent,
        }
    }
}

impl<F> Future for InstrumentedFuture<F>
where
    F: Future,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let parent = *this.parent;
        ASYNC_CALL_STACK.with(|stack_cell| {
            let stack = unsafe { &mut *stack_cell.get() };
            assert!(!stack.is_empty());
            let depth = (stack.len().saturating_sub(1)) as u32;
            match this.span_id {
                Some(span_id) => {
                    stack.push(*span_id);
                }
                None => {
                    // Begin the async span and store the span ID
                    let span_id = on_begin_async_scope(this.desc, parent, depth);
                    stack.push(span_id);
                    *this.span_id = Some(span_id);
                }
            }
            let res = match this.future.poll(cx) {
                Poll::Ready(output) => {
                    // End the async span when the future completes
                    if let Some(span_id) = *this.span_id {
                        on_end_async_scope(span_id, parent, this.desc, depth);
                    }
                    Poll::Ready(output)
                }
                Poll::Pending => Poll::Pending,
            };
            stack.pop();
            res
        })
    }
}

/// A wrapper that instruments a future with named async span tracing
#[pin_project]
pub struct InstrumentedNamedFuture<F> {
    #[pin]
    future: F,
    span_location: &'static SpanLocation,
    name: &'static str,
    span_id: Option<u64>,
    /// Parent span ID captured at future creation time
    parent: u64,
}

impl<F> InstrumentedNamedFuture<F> {
    /// Create a new instrumented named future
    pub fn new(future: F, span_location: &'static SpanLocation, name: &'static str) -> Self {
        let parent = ASYNC_CALL_STACK.with(|stack_cell| {
            let stack = unsafe { &*stack_cell.get() };
            assert!(!stack.is_empty());
            stack[stack.len() - 1]
        });
        Self {
            future,
            span_location,
            name,
            span_id: None,
            parent,
        }
    }
}

impl<F> Future for InstrumentedNamedFuture<F>
where
    F: Future,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let parent = *this.parent;
        ASYNC_CALL_STACK.with(|stack_cell| {
            let stack = unsafe { &mut *stack_cell.get() };
            assert!(!stack.is_empty());
            let depth = (stack.len().saturating_sub(1)) as u32;
            match this.span_id {
                Some(span_id) => {
                    stack.push(*span_id);
                }
                None => {
                    // Begin the async named span and store the span ID
                    let span_id =
                        on_begin_async_named_scope(this.span_location, this.name, parent, depth);
                    stack.push(span_id);
                    *this.span_id = Some(span_id);
                }
            }
            let res = match this.future.poll(cx) {
                Poll::Ready(output) => {
                    // End the async named span when the future completes
                    if let Some(span_id) = *this.span_id {
                        on_end_async_named_scope(
                            span_id,
                            parent,
                            this.span_location,
                            this.name,
                            depth,
                        );
                    }
                    Poll::Ready(output)
                }
                Poll::Pending => Poll::Pending,
            };
            stack.pop();
            res
        })
    }
}
