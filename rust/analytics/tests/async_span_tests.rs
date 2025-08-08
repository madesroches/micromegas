use micromegas_tracing::prelude::*;
use pin_project::pin_project;
use rand::Rng;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::time::sleep;

trait InstrumentFuture: Future + Sized {
    fn instrument(self, name: &str) -> InstrumentedFuture<Self> {
        InstrumentedFuture::new(self, name)
    }
}

impl<F: Future> InstrumentFuture for F {}

#[pin_project]
struct InstrumentedFuture<F> {
    #[pin]
    future: F,
    name: String,
    started: bool,
}

impl<F> InstrumentedFuture<F> {
    fn new(future: F, name: &str) -> Self {
        Self {
            future,
            name: name.to_string(),
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
            eprintln!("Starting future: {}", this.name);
            *this.started = true;
        }
        match this.future.poll(cx) {
            Poll::Ready(output) => {
                eprintln!("Finished future: {}", this.name);
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
    manual_inner().instrument("manual_inner_1").await;
    manual_inner().instrument("manual_inner_2").await;
}

#[test]
fn async_span_manual_instrumentation() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");
    runtime.block_on(manual_outer().instrument("manual_outer"));
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
