//! Manual async span instrumentation using InstrumentedFuture wrapper

use crate::dispatch::{
    on_begin_async_scope, on_end_async_named_scope_tagged, on_end_async_scope,
    on_end_async_scope_tagged,
};
use crate::property_set::PropertySet;
use crate::spans::{
    DynSpanName, SpanLocation, SpanMetadata, SpanName, SpanProperties, StaticSpanName,
};
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

/// Runs when an instrumented future completes, to add properties to the span's end event from the
/// future's output.
pub trait SpanEndHook<O> {
    #[doc(hidden)]
    fn properties_at_end(self, output: &O) -> Option<SpanProperties>;
}

/// The end hook of a future that carries no properties: its span ends plainly.
pub struct NoHook;

impl<O> SpanEndHook<O> for NoHook {
    #[inline(always)]
    fn properties_at_end(self, _output: &O) -> Option<SpanProperties> {
        None
    }
}

impl<O, H> SpanEndHook<O> for H
where
    H: FnOnce(&O, &mut SpanProperties),
{
    fn properties_at_end(self, output: &O) -> Option<SpanProperties> {
        let mut properties = SpanProperties::new(PropertySet::empty());
        self(output, &mut properties);
        if properties.is_empty() {
            None
        } else {
            Some(properties)
        }
    }
}

/// Trait for adding instrumentation to futures
pub trait InstrumentFuture: Future + Sized {
    /// Instrument this future with the given span metadata
    fn instrument(self, span_desc: &'static SpanMetadata) -> InstrumentedFuture<Self> {
        InstrumentedFuture::new(self, span_desc)
    }

    /// Instruments this future, and lets `on_end` add properties to the end event from the
    /// output. A hook that sets nothing leaves the plain end event.
    ///
    /// ```
    /// use micromegas_tracing::prelude::*;
    /// use micromegas_tracing::spans::InstrumentFuture as _;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// static_span_desc!(FETCH, "fetch");
    /// let rows = async { vec![1, 2, 3] }
    ///     .instrument_with(&FETCH, |rows, span| span.set_property("rows", rows.len()))
    ///     .await;
    /// # assert_eq!(rows.len(), 3);
    /// # }
    /// ```
    fn instrument_with<H>(
        self,
        span_desc: &'static SpanMetadata,
        on_end: H,
    ) -> InstrumentedFuture<Self, H>
    where
        H: FnOnce(&Self::Output, &mut SpanProperties),
    {
        InstrumentedFuture::with_hook(self, span_desc, on_end)
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

    /// Internal method for runtime-named instrumentation - do not use directly.
    /// Use the `instrument_named_dyn!` macro for method-like syntax instead.
    #[doc(hidden)]
    fn __instrument_named_dyn_internal<N: AsRef<str>>(
        self,
        span_location: &'static SpanLocation,
        name: N,
    ) -> InstrumentedNamedFuture<Self, DynSpanName<N>> {
        InstrumentedNamedFuture::with_name(self, span_location, DynSpanName(name))
    }

    /// Internal method for named instrumentation with an end hook - do not use directly.
    #[doc(hidden)]
    fn __instrument_named_with_internal<H>(
        self,
        span_location: &'static SpanLocation,
        name: &'static str,
        on_end: H,
    ) -> InstrumentedNamedFuture<Self, StaticSpanName, H>
    where
        H: FnOnce(&Self::Output, &mut SpanProperties),
    {
        InstrumentedNamedFuture::with_name_and_hook(
            self,
            span_location,
            StaticSpanName(name),
            on_end,
        )
    }

    /// Internal method for runtime-named instrumentation with an end hook - do not use directly.
    #[doc(hidden)]
    fn __instrument_named_dyn_with_internal<N: AsRef<str>, H>(
        self,
        span_location: &'static SpanLocation,
        name: N,
        on_end: H,
    ) -> InstrumentedNamedFuture<Self, DynSpanName<N>, H>
    where
        H: FnOnce(&Self::Output, &mut SpanProperties),
    {
        InstrumentedNamedFuture::with_name_and_hook(self, span_location, DynSpanName(name), on_end)
    }
}

impl<F: Future> InstrumentFuture for F {}

/// A wrapper that instruments a future with async span tracing
#[pin_project]
pub struct InstrumentedFuture<F, H = NoHook> {
    #[pin]
    future: F,
    desc: &'static SpanMetadata,
    on_end: Option<H>,
    span_id: Option<u64>,
    /// Parent span ID captured at future creation time
    parent: u64,
    /// Depth captured at future creation time
    depth: u32,
}

impl<F> InstrumentedFuture<F> {
    /// Create a new instrumented future
    pub fn new(future: F, desc: &'static SpanMetadata) -> Self {
        Self::with_hook(future, desc, NoHook)
    }
}

impl<F, H> InstrumentedFuture<F, H> {
    /// Create a new instrumented future whose end event carries the properties `on_end` sets.
    pub fn with_hook(future: F, desc: &'static SpanMetadata, on_end: H) -> Self {
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
            on_end: Some(on_end),
            span_id: None,
            parent,
            depth,
        }
    }
}

impl<F, H> Future for InstrumentedFuture<F, H>
where
    F: Future,
    H: SpanEndHook<F::Output>,
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
                        let properties = this
                            .on_end
                            .take()
                            .and_then(|hook| hook.properties_at_end(&output));
                        match properties {
                            Some(properties) => properties.with_dyn(|set, dyn_properties| {
                                on_end_async_scope_tagged(
                                    span_id,
                                    parent,
                                    this.desc,
                                    depth,
                                    set,
                                    dyn_properties,
                                );
                            }),
                            None => on_end_async_scope(span_id, parent, this.desc, depth),
                        }
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
pub struct InstrumentedNamedFuture<F, N = StaticSpanName, H = NoHook> {
    #[pin]
    future: F,
    span_location: &'static SpanLocation,
    name: N,
    on_end: Option<H>,
    span_id: Option<u64>,
    /// Parent span ID captured at future creation time
    parent: u64,
    /// Depth captured at future creation time
    depth: u32,
}

impl<F> InstrumentedNamedFuture<F> {
    /// Create a new instrumented named future
    pub fn new(future: F, span_location: &'static SpanLocation, name: &'static str) -> Self {
        Self::with_name(future, span_location, StaticSpanName(name))
    }
}

impl<F, N: SpanName> InstrumentedNamedFuture<F, N> {
    /// Create a new instrumented named future
    pub fn with_name(future: F, span_location: &'static SpanLocation, name: N) -> Self {
        Self::with_name_and_hook(future, span_location, name, NoHook)
    }
}

impl<F, N: SpanName, H> InstrumentedNamedFuture<F, N, H> {
    /// Create a new instrumented named future whose end event carries the properties `on_end`
    /// sets.
    pub fn with_name_and_hook(
        future: F,
        span_location: &'static SpanLocation,
        name: N,
        on_end: H,
    ) -> Self {
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
            on_end: Some(on_end),
            span_id: None,
            parent,
            depth,
        }
    }
}

impl<F, N: SpanName, H> Future for InstrumentedNamedFuture<F, N, H>
where
    F: Future,
    H: SpanEndHook<F::Output>,
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
                    let span_id = this.name.begin_async(this.span_location, parent, depth);
                    stack.push(span_id);
                    *this.span_id = Some(span_id);
                }
            }
            let res = match this.future.poll(cx) {
                Poll::Ready(output) => {
                    // End the async named span when the future completes
                    if let Some(span_id) = *this.span_id {
                        let properties = this
                            .on_end
                            .take()
                            .and_then(|hook| hook.properties_at_end(&output));
                        match properties {
                            Some(properties) => properties.with_dyn(|set, dyn_properties| {
                                on_end_async_named_scope_tagged(
                                    span_id,
                                    parent,
                                    this.span_location,
                                    this.name.as_str(),
                                    depth,
                                    set,
                                    dyn_properties,
                                );
                            }),
                            None => this
                                .name
                                .end_async(span_id, parent, this.span_location, depth),
                        }
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
