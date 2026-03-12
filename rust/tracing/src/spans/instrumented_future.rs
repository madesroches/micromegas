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

/// A future wrapper that establishes a span context on every poll.
///
/// Unlike an RAII guard, this pushes the parent span ID onto the
/// thread-local async call stack before each poll and pops it after,
/// which is correct across yield points and executor thread migration.
///
/// The stack is padded to match the parent's depth so that
/// `InstrumentedFuture::new()` inside the spawned task computes
/// `depth = stack.len() - 1 = parent_depth + 1`, preserving the
/// logical nesting across spawn boundaries.
#[pin_project]
pub struct SpanContextFuture<F> {
    #[pin]
    future: F,
    parent_span: u64,
    parent_depth: u32,
}

impl<F: Future> Future for SpanContextFuture<F> {
    type Output = F::Output;

    #[inline]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        ASYNC_CALL_STACK.with(|stack_cell| {
            let stack = unsafe { &mut *stack_cell.get() };
            let saved_len = stack.len();
            // Pad stack so that children see depth = parent_depth.
            // At the spawn point the stack had length parent_depth + 1;
            // we recreate that, then push parent_span on top.
            let target_len = *this.parent_depth as usize + 1;
            while stack.len() < target_len.saturating_sub(1) {
                stack.push(0);
            }
            stack.push(*this.parent_span);
            let res = this.future.poll(cx);
            stack.truncate(saved_len);
            res
        })
    }
}

/// Spawns a future on the tokio runtime while preserving the current span context.
///
/// This is a wrapper around `tokio::spawn` that captures the current span ID
/// before spawning and establishes it as the parent context in the spawned task.
/// This ensures that instrumented async functions called within the spawned task
/// will correctly report the spawning context as their parent.
///
/// The parent span ID is pushed onto the thread-local async call stack before
/// each poll and popped after, so it works correctly across yield points and
/// executor thread migration.
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
    let (parent_span, parent_depth) = ASYNC_CALL_STACK.with(|stack_cell| {
        let stack = unsafe { &*stack_cell.get() };
        (
            stack.last().copied().unwrap_or(0),
            (stack.len().saturating_sub(1)) as u32,
        )
    });
    tokio::spawn(SpanContextFuture {
        future,
        parent_span,
        parent_depth,
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
    /// Depth captured at future creation time
    depth: u32,
}

impl<F> InstrumentedFuture<F> {
    /// Create a new instrumented future
    pub fn new(future: F, desc: &'static SpanMetadata) -> Self {
        let (parent, depth) = ASYNC_CALL_STACK.with(|stack_cell| {
            let stack = unsafe { &*stack_cell.get() };
            assert!(!stack.is_empty());
            (
                stack[stack.len() - 1],
                (stack.len().saturating_sub(1)) as u32,
            )
        });
        Self {
            future,
            desc,
            span_id: None,
            parent,
            depth,
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
        let depth = *this.depth;
        ASYNC_CALL_STACK.with(|stack_cell| {
            let stack = unsafe { &mut *stack_cell.get() };
            assert!(!stack.is_empty());
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
    /// Depth captured at future creation time
    depth: u32,
}

impl<F> InstrumentedNamedFuture<F> {
    /// Create a new instrumented named future
    pub fn new(future: F, span_location: &'static SpanLocation, name: &'static str) -> Self {
        let (parent, depth) = ASYNC_CALL_STACK.with(|stack_cell| {
            let stack = unsafe { &*stack_cell.get() };
            assert!(!stack.is_empty());
            (
                stack[stack.len() - 1],
                (stack.len().saturating_sub(1)) as u32,
            )
        });
        Self {
            future,
            span_location,
            name,
            span_id: None,
            parent,
            depth,
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
        let depth = *this.depth;
        ASYNC_CALL_STACK.with(|stack_cell| {
            let stack = unsafe { &mut *stack_cell.get() };
            assert!(!stack.is_empty());
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
