use pin_project::pin_project;
use rand::Rng;
use std::time::Duration;
use tokio::time::sleep;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

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

async fn inner() {
    let ms = rand::thread_rng().gen_range(0..=1000);
    eprintln!("wainting for {ms} ms");
    sleep(Duration::from_millis(ms)).await;
}

async fn outer() {
    inner().instrument("inner_1").await;
    inner().instrument("inner_2").await;
}

#[test]
fn async_span_smoke() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    runtime.block_on(outer());
}
