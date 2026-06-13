use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::{Router, routing::get};
use tokio::sync::Notify;

use micromegas::servers::shutdown::serve_axum_with_graceful_shutdown;

async fn bind_random() -> (tokio::net::TcpListener, SocketAddr) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind random port");
    let addr = listener.local_addr().expect("local_addr");
    (listener, addr)
}

/// Handler sleeps ~300ms; dispatch a request, trigger shutdown while it's in
/// flight; assert the client gets 200 and the serve call returns Ok before the
/// grace period.
#[tokio::test]
async fn axum_drain_completes() {
    let (listener, addr) = bind_random().await;
    let notify = Arc::new(Notify::new());
    let notify2 = notify.clone();
    // Used to synchronize: trigger shutdown only after handler has started.
    let handler_started = Arc::new(Notify::new());
    let handler_started2 = handler_started.clone();

    let app = Router::new().route(
        "/slow",
        get(move || {
            let started = handler_started2.clone();
            async move {
                started.notify_one();
                tokio::time::sleep(Duration::from_millis(300)).await;
                "ok"
            }
        }),
    );

    let serve = tokio::spawn(serve_axum_with_graceful_shutdown(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
        async move { notify2.notified().await },
        Duration::from_secs(5),
    ));

    tokio::time::sleep(Duration::from_millis(50)).await;

    let url = format!("http://{addr}/slow");
    let client_task =
        tokio::spawn(async move { reqwest::get(url).await.expect("request").status().as_u16() });

    // Trigger shutdown only after the handler has started executing
    handler_started.notified().await;
    notify.notify_one();

    let status = client_task.await.expect("client task");
    assert_eq!(status, 200);

    let res = serve.await.expect("serve task");
    assert!(res.is_ok());
}

/// Handler sleeps longer than the grace period; assert serve returns shortly
/// after grace elapses rather than waiting for the handler to finish.
#[tokio::test]
async fn axum_grace_cap_enforced() {
    let (listener, addr) = bind_random().await;
    let notify = Arc::new(Notify::new());
    let notify2 = notify.clone();

    let app = Router::new().route(
        "/long",
        get(|| async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            "ok"
        }),
    );

    let serve = tokio::spawn(serve_axum_with_graceful_shutdown(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
        async move { notify2.notified().await },
        Duration::from_millis(200),
    ));

    tokio::time::sleep(Duration::from_millis(50)).await;

    let _client = tokio::spawn(async move {
        let _ = reqwest::get(format!("http://{addr}/long")).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let before = std::time::Instant::now();
    notify.notify_one();
    let res = serve.await.expect("serve task");
    let elapsed = before.elapsed();

    assert!(res.is_ok());
    // Should return well within 2s (grace=200ms + overhead), not 10s
    assert!(elapsed < Duration::from_secs(2), "elapsed={elapsed:?}");
}

/// After triggering shutdown, new connection attempts should be refused
/// (axum stops accepting). Assertion is loose because the exact timing is
/// implementation-dependent.
#[tokio::test]
async fn new_connections_refused_after_signal() {
    let (listener, addr) = bind_random().await;
    let notify = Arc::new(Notify::new());
    let notify2 = notify.clone();

    let app = Router::new().route("/", get(|| async { "ok" }));

    let serve = tokio::spawn(serve_axum_with_graceful_shutdown(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
        async move { notify2.notified().await },
        Duration::from_secs(5),
    ));

    tokio::time::sleep(Duration::from_millis(50)).await;
    notify.notify_one();

    // Drain and exit
    let res = serve.await.expect("serve task");
    assert!(res.is_ok());

    // After the server has stopped, connections must be refused
    let result = tokio::net::TcpStream::connect(addr).await;
    assert!(
        result.is_err(),
        "expected connection refused after server stopped"
    );
}

/// run_tasks_forever with a stub task that records completion; trigger
/// shutdown mid-task; assert the loop waits for the task to finish before
/// returning.
#[tokio::test]
async fn cron_loop_drains() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use async_trait::async_trait;
    use chrono::{DateTime, TimeDelta, Utc};
    use micromegas::servers::cron_task::{CronTask, TaskCallback};
    use micromegas::servers::maintenance::run_tasks_forever;

    let finished = Arc::new(AtomicBool::new(false));
    let finished2 = finished.clone();

    struct SlowTask {
        finished: Arc<AtomicBool>,
    }

    #[async_trait]
    impl TaskCallback for SlowTask {
        async fn run(&self, _: DateTime<Utc>) -> anyhow::Result<()> {
            tokio::time::sleep(Duration::from_millis(300)).await;
            self.finished.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    // offset = -period so next_run = start of current hour window <= now
    let task = CronTask::new(
        "slow_task".to_string(),
        TimeDelta::seconds(3600),
        TimeDelta::seconds(-3600),
        Arc::new(SlowTask {
            finished: finished2,
        }),
    )
    .expect("create task");

    let notify = Arc::new(Notify::new());
    let notify2 = notify.clone();

    let runner = tokio::spawn(run_tasks_forever(vec![task], 1, async move {
        notify2.notified().await
    }));

    // Give the task time to start executing
    tokio::time::sleep(Duration::from_millis(50)).await;
    notify.notify_one();

    runner.await.expect("runner task");
    assert!(
        finished.load(Ordering::SeqCst),
        "task should have completed before loop returned"
    );
}
