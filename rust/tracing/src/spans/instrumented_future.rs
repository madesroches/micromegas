//! Manual async span instrumentation using InstrumentedFuture wrapper

use crate::dispatch::{on_begin_async_scope, on_end_async_scope};
use crate::spans::SpanMetadata;
use pin_project::pin_project;
use std::cell::UnsafeCell;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

thread_local! {
    static ASYNC_CALL_STACK: UnsafeCell<Vec<u64>> = UnsafeCell ::new(vec![0]);
}

/// Trait for adding instrumentation to futures
pub trait InstrumentFuture: Future + Sized {
    /// Instrument this future with the given span metadata
    fn instrument(self, span_desc: &'static SpanMetadata) -> InstrumentedFuture<Self> {
        InstrumentedFuture::new(self, span_desc)
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
}

impl<F> InstrumentedFuture<F> {
    /// Create a new instrumented future
    pub fn new(future: F, desc: &'static SpanMetadata) -> Self {
        Self {
            future,
            desc,
            span_id: None,
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
        ASYNC_CALL_STACK.with(|stack_cell| {
            let stack = unsafe { &mut *stack_cell.get() };
            assert!(!stack.is_empty());
            let parent = stack[stack.len() - 1];
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
