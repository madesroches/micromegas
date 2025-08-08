use rand::Rng;
use std::time::Duration;
use tokio::time::sleep;

async fn inner() {
    let ms = rand::thread_rng().gen_range(0..=1000);
    eprintln!("wainting for {ms} ms");
    sleep(Duration::from_millis(ms)).await;
}

async fn outer() {
    inner().await;
    inner().await
}

#[test]
fn async_span_smoke() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    runtime.block_on(outer());
}
