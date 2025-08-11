use micromegas_tracing::spans::SpanMetadata;
use micromegas_tracing::{prelude::*, static_span_desc};
use pin_project::pin_project;
use rand::Rng;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::time::sleep;

trait InstrumentFuture: Future + Sized {
    fn instrument(self, span_desc: &'static SpanMetadata) -> InstrumentedFuture<Self> {
        InstrumentedFuture::new(self, span_desc)
    }
}

impl<F: Future> InstrumentFuture for F {}

#[pin_project]
struct InstrumentedFuture<F> {
    #[pin]
    future: F,
    desc: &'static SpanMetadata,
    started: bool,
}

impl<F> InstrumentedFuture<F> {
    fn new(future: F, desc: &'static SpanMetadata) -> Self {
        Self {
            future,
            desc,
            started: false,
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
        if !*this.started {
            eprintln!("Starting future: {:?}", this.desc);
            *this.started = true;
        }
        match this.future.poll(cx) {
            Poll::Ready(output) => {
                eprintln!("Finished future: {:?}", this.desc);
                Poll::Ready(output)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

async fn manual_inner() {
    let ms = rand::thread_rng().gen_range(0..=1000);
    eprintln!("wainting for {ms} ms");
    sleep(Duration::from_millis(ms)).await;
}

async fn manual_outer() {
    static_span_desc!(INNER1_DESC, "manual_inner_1");
    manual_inner().instrument(&INNER1_DESC).await;
    static_span_desc!(INNER2_DESC, "manual_inner_2");
    manual_inner().instrument(&INNER2_DESC).await;
}

#[test]
fn async_span_manual_instrumentation() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");
    static_span_desc!(OUTER_DESC, "manual_outer");
    runtime.block_on(manual_outer().instrument(&OUTER_DESC));
}

#[span_fn]
async fn macro_inner() {
    let ms = rand::thread_rng().gen_range(0..=1000);
    eprintln!("waiting for {ms} ms");
    sleep(Duration::from_millis(ms)).await;
}

#[span_fn]
async fn macro_outer() {
    macro_inner().await;
    macro_inner().await;
}

#[span_fn]
fn sync_function() {
    eprintln!("This is a sync function");
    std::thread::sleep(Duration::from_millis(100));
}

#[test]
fn async_span_macro() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    runtime.block_on(macro_outer());
}

#[test]
fn sync_span_macro() {
    sync_function();
}
