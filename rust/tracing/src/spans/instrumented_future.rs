//! Manual async span instrumentation using InstrumentedFuture wrapper

use crate::dispatch::{on_begin_async_scope, on_end_async_scope};
use crate::spans::SpanMetadata;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

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
        if this.span_id.is_none() {
            // Begin the async span and store the span ID
            let span_id = on_begin_async_scope(this.desc);
            *this.span_id = Some(span_id);
        }
        match this.future.poll(cx) {
            Poll::Ready(output) => {
                // End the async span when the future completes
                if let Some(span_id) = *this.span_id {
                    on_end_async_scope(span_id, this.desc);
                }
                Poll::Ready(output)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
