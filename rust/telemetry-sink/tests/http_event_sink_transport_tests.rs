//! Transport-hardening tests for `HttpEventSink`: priority ordering, lazy
//! stream metadata, graded byte-budget dropping, and shutdown draining.
//!
//! These exercise the sink only through its public `EventSink` trait
//! methods, against a minimal local HTTP mock server, so no internal
//! plumbing needs to be exposed for testing.

use micromegas_telemetry_sink::http_event_sink::{HttpEventSink, HttpSinkConfig};
use micromegas_telemetry_sink::request_decorator::TrivialRequestDecorator;
use micromegas_tracing::event::{EventSink, EventStream, TracingBlock};
use micromegas_tracing::images::ImageStream;
use micromegas_tracing::logs::{LogBlock, LogStream};
use micromegas_tracing::metrics::{MetricsBlock, MetricsStream};
use micromegas_tracing::process_info::ProcessInfo;
use micromegas_tracing::spans::{ThreadBlock, ThreadStream};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio_retry2::strategy::ExponentialBackoff;

#[derive(Debug, Clone)]
struct CapturedRequest {
    path: String,
    body: Vec<u8>,
}

struct MockServer {
    addr: String,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    // Held at 0 permits when `gate_first` is set; the first accepted
    // connection blocks on it until the test calls `gate.add_permits(1)`.
    gate: Arc<Semaphore>,
}

async fn handle_connection(
    mut socket: tokio::net::TcpStream,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    gate: Option<Arc<Semaphore>>,
) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = socket.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
    };
    let header_str = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let path = header_str
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("")
        .to_string();
    let content_length: usize = header_str
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            if k.trim().eq_ignore_ascii_case("content-length") {
                v.trim().parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    let body_start = header_end + 4;
    while buf.len() < body_start + content_length {
        let n = socket.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body_end = (body_start + content_length).min(buf.len());
    let body = buf[body_start..body_end].to_vec();

    requests
        .lock()
        .unwrap()
        .push(CapturedRequest { path, body });

    if let Some(gate) = gate {
        gate.acquire().await.expect("gate acquire").forget();
    }

    let _ = socket
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await;
    let _ = socket.shutdown().await;
}

/// Spawns a minimal local HTTP server capturing request paths and bodies in
/// arrival order. If `gate_first` is set, the very first accepted connection
/// blocks (before recording itself and responding) until the returned
/// semaphore is given a permit.
async fn spawn_mock_server(gate_first: bool) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server");
    let addr = listener.local_addr().expect("local_addr");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let gate = Arc::new(Semaphore::new(0));
    let requests_task = requests.clone();
    let gate_task = gate.clone();
    tokio::spawn(async move {
        let first = AtomicBool::new(true);
        loop {
            let (socket, _) = match listener.accept().await {
                Ok(x) => x,
                Err(_) => break,
            };
            let requests = requests_task.clone();
            let is_first = gate_first && first.swap(false, Ordering::SeqCst);
            let gate = is_first.then(|| gate_task.clone());
            tokio::spawn(handle_connection(socket, requests, gate));
        }
    });
    MockServer {
        addr: format!("http://{addr}"),
        requests,
        gate,
    }
}

fn body_contains(body: &[u8], needle: &str) -> bool {
    let needle = needle.as_bytes();
    !needle.is_empty() && body.windows(needle.len()).any(|w| w == needle)
}

async fn wait_until(mut check: impl FnMut() -> bool, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while !check() {
        if start.elapsed() > timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    true
}

fn make_process_info() -> Arc<ProcessInfo> {
    Arc::new(ProcessInfo {
        process_id: uuid::Uuid::new_v4(),
        exe: "test".into(),
        username: "test".into(),
        realname: "test".into(),
        computer: "test".into(),
        distro: "test".into(),
        cpu_brand: "test".into(),
        tsc_frequency: 1_000_000_000,
        start_time: chrono::Utc::now(),
        start_ticks: 0,
        parent_process_id: None,
        properties: HashMap::new(),
    })
}

fn make_log_stream_and_block(process_id: uuid::Uuid) -> (LogStream, Arc<LogBlock>) {
    let stream = EventStream::new(64 * 1024, process_id, &[], HashMap::new());
    let mut block = LogBlock::new(1024, process_id, stream.stream_id(), 0);
    block.close();
    (stream, Arc::new(block))
}

fn make_metrics_stream_and_block(process_id: uuid::Uuid) -> (MetricsStream, Arc<MetricsBlock>) {
    let stream = EventStream::new(64 * 1024, process_id, &[], HashMap::new());
    let mut block = MetricsBlock::new(1024, process_id, stream.stream_id(), 0);
    block.close();
    (stream, Arc::new(block))
}

fn make_thread_stream_and_block(process_id: uuid::Uuid) -> (ThreadStream, Arc<ThreadBlock>) {
    let stream = EventStream::new(64 * 1024, process_id, &[], HashMap::new());
    let mut block = ThreadBlock::new(1024, process_id, stream.stream_id(), 0);
    block.close();
    (stream, Arc::new(block))
}

fn make_image_stream(process_id: uuid::Uuid) -> ImageStream {
    EventStream::new(64 * 1024, process_id, &[], HashMap::new())
}

fn make_sink(addr: &str, config: HttpSinkConfig) -> HttpEventSink {
    HttpEventSink::new(
        addr,
        config,
        Box::new(|| Arc::new(TrivialRequestDecorator {})),
    )
}

/// Metadata is drained before Logs, Metrics, and Traces; within a priority
/// class items are drained FIFO. A stream with no processed block never gets
/// its `insert_stream` sent (lazy stream metadata).
#[tokio::test]
async fn priority_order_and_lazy_stream_metadata() {
    let server = spawn_mock_server(true).await;
    let process_info = make_process_info();
    let process_id = process_info.process_id;

    let sink = make_sink(
        &server.addr,
        HttpSinkConfig {
            max_in_flight_requests: 1,
            ..Default::default()
        },
    );

    // Enqueued first; the worker immediately dequeues and sends it, but the
    // mock server holds the response back until we release the gate below.
    sink.on_startup(process_info);

    let (log_stream, log_block) = make_log_stream_and_block(process_id);
    let (metrics_stream, metrics_block) = make_metrics_stream_and_block(process_id);
    let (thread_stream, thread_block) = make_thread_stream_and_block(process_id);
    let idle_image_stream = make_image_stream(process_id);

    sink.on_init_log_stream(&log_stream);
    sink.on_init_metrics_stream(&metrics_stream);
    sink.on_init_thread_stream(&thread_stream);
    sink.on_init_image_stream(&idle_image_stream);

    // With max_in_flight_requests=1 and the sole permit held by the gated
    // `insert_process` send, none of these can be dequeued yet: they queue up
    // in enqueue order within their priority class.
    sink.on_process_thread_block(thread_block); // Metadata(thread stream) + Traces
    sink.on_process_metrics_block(metrics_block); // Metadata(metrics stream) + Metrics
    sink.on_process_log_block(log_block); // Metadata(log stream) + Logs

    // Let the process request arrive at the mock server, then release it.
    assert!(
        wait_until(
            || !server.requests.lock().unwrap().is_empty(),
            Duration::from_secs(2)
        )
        .await,
        "insert_process never reached the mock server"
    );
    server.gate.add_permits(1);

    assert!(
        wait_until(
            || server.requests.lock().unwrap().len() >= 7,
            Duration::from_secs(2)
        )
        .await,
        "not all expected requests arrived: {:?}",
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| &r.path)
            .collect::<Vec<_>>()
    );

    let requests = server.requests.lock().unwrap().clone();
    assert_eq!(requests[0].path, "/ingestion/insert_process");

    for req in &requests[1..4] {
        assert_eq!(req.path, "/ingestion/insert_stream");
    }

    assert_eq!(requests[4].path, "/ingestion/insert_block");
    assert!(body_contains(
        &requests[4].body,
        &log_stream.stream_id().to_string()
    ));

    assert_eq!(requests[5].path, "/ingestion/insert_block");
    assert!(body_contains(
        &requests[5].body,
        &metrics_stream.stream_id().to_string()
    ));

    assert_eq!(requests[6].path, "/ingestion/insert_block");
    assert!(body_contains(
        &requests[6].body,
        &thread_stream.stream_id().to_string()
    ));

    // The idle image stream never emitted anything on the wire.
    let idle_id = idle_image_stream.stream_id().to_string();
    assert!(!requests.iter().any(|r| body_contains(&r.body, &idle_id)));
}

/// `on_shutdown` waits for the final drain: every item queued before shutdown
/// is submitted and completes before it returns.
#[tokio::test]
async fn shutdown_drains_pending_items() {
    let server = spawn_mock_server(false).await;
    let process_info = make_process_info();
    let process_id = process_info.process_id;

    let sink = make_sink(&server.addr, HttpSinkConfig::default());

    sink.on_startup(process_info);
    let (log_stream, log_block) = make_log_stream_and_block(process_id);
    let (metrics_stream, metrics_block) = make_metrics_stream_and_block(process_id);
    sink.on_init_log_stream(&log_stream);
    sink.on_init_metrics_stream(&metrics_stream);
    sink.on_process_log_block(log_block);
    sink.on_process_metrics_block(metrics_block);

    // on_shutdown blocks (background OS thread) until the final drain
    // completes; run it on a blocking thread so the test's async runtime
    // (hosting the mock server) keeps making progress concurrently.
    tokio::task::spawn_blocking(move || sink.on_shutdown())
        .await
        .expect("on_shutdown task");

    // process + insert_stream(log) + insert_stream(metrics) + block(log) + block(metrics)
    assert_eq!(server.requests.lock().unwrap().len(), 5);
}

/// A short-lived process must not freeze on shutdown when the ingestion
/// service is unresponsive (accepts the TCP connection but never replies) —
/// as opposed to merely offline, which fails fast with a connection error.
/// Without a per-request timeout, the single in-flight send would hang
/// forever, and since `Drop for HttpEventSink` joins the worker thread, that
/// would freeze the whole process at exit.
#[tokio::test]
async fn shutdown_does_not_freeze_when_ingestion_is_unresponsive() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        // Accept connections but never read or respond; keep them alive so
        // the client can't observe a closed connection either.
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });

    let one_attempt = || ExponentialBackoff::from_millis(10).take(1);
    let sink = make_sink(
        &format!("http://{addr}"),
        HttpSinkConfig {
            request_timeout: Duration::from_millis(200),
            retry_by_priority: [one_attempt(), one_attempt(), one_attempt(), one_attempt()],
            ..Default::default()
        },
    );
    sink.on_startup(make_process_info());

    let shutdown = tokio::task::spawn_blocking(move || {
        let start = std::time::Instant::now();
        sink.on_shutdown();
        start.elapsed()
    });
    let elapsed = tokio::time::timeout(Duration::from_secs(10), shutdown)
        .await
        .expect("on_shutdown hung past the test's outer timeout")
        .expect("shutdown task");

    assert!(
        elapsed < Duration::from_secs(3),
        "on_shutdown took too long against an unresponsive server: {elapsed:?}"
    );
}

/// Once the queue holds at least `max_queue_bytes`, new Traces items are
/// dropped while Logs/Metrics/Metadata keep flowing.
#[tokio::test]
async fn soft_cap_sheds_traces_first() {
    let server = spawn_mock_server(false).await;
    let process_info = make_process_info();
    let process_id = process_info.process_id;

    let sink = make_sink(
        &server.addr,
        HttpSinkConfig {
            max_queue_bytes: 0,
            hard_queue_bytes: 10_000_000,
            ..Default::default()
        },
    );

    sink.on_startup(process_info);
    let (thread_stream, thread_block) = make_thread_stream_and_block(process_id);
    let (log_stream, log_block) = make_log_stream_and_block(process_id);
    sink.on_init_thread_stream(&thread_stream);
    sink.on_init_log_stream(&log_stream);
    sink.on_process_thread_block(thread_block); // Traces: dropped (0 >= soft(0))
    sink.on_process_log_block(log_block); // Logs: kept (0 < hard)

    assert!(
        wait_until(
            || {
                let requests = server.requests.lock().unwrap();
                body_contains_any(&requests, &log_stream.stream_id().to_string())
                    && requests.iter().any(|r| {
                        r.path == "/ingestion/insert_block"
                            && body_contains(&r.body, &log_stream.stream_id().to_string())
                    })
            },
            Duration::from_secs(2)
        )
        .await,
        "expected log block to still be delivered"
    );

    // The thread stream's metadata is still sent (Metadata is never
    // dropped), but its block never arrives. With the default
    // max_in_flight_requests > 1, dequeue order doesn't guarantee arrival
    // order at the mock server, so wait for it rather than asserting
    // immediately.
    let thread_id = thread_stream.stream_id().to_string();
    assert!(
        wait_until(
            || {
                server.requests.lock().unwrap().iter().any(|r| {
                    r.path == "/ingestion/insert_stream" && body_contains(&r.body, &thread_id)
                })
            },
            Duration::from_secs(2)
        )
        .await,
        "expected thread stream metadata to still be sent"
    );

    let requests = server.requests.lock().unwrap();
    assert!(
        !requests
            .iter()
            .any(|r| r.path == "/ingestion/insert_block" && body_contains(&r.body, &thread_id))
    );
}

/// Once the queue holds at least `hard_queue_bytes`, Logs/Metrics are dropped
/// too; Metadata is still never dropped.
#[tokio::test]
async fn hard_cap_also_sheds_logs_and_metrics_but_never_metadata() {
    let server = spawn_mock_server(false).await;
    let process_info = make_process_info();
    let process_id = process_info.process_id;

    let sink = make_sink(
        &server.addr,
        HttpSinkConfig {
            max_queue_bytes: 0,
            hard_queue_bytes: 0,
            ..Default::default()
        },
    );

    sink.on_startup(process_info.clone());
    let (log_stream, log_block) = make_log_stream_and_block(process_id);
    sink.on_init_log_stream(&log_stream);
    sink.on_process_log_block(log_block);

    // Give the (dropped) log block a chance to have been wrongly sent, then
    // confirm only the metadata made it through.
    assert!(
        wait_until(
            || server.requests.lock().unwrap().len() >= 2,
            Duration::from_secs(2)
        )
        .await,
        "expected insert_process + insert_stream, got: {:?}",
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| &r.path)
            .collect::<Vec<_>>()
    );
    tokio::time::sleep(Duration::from_millis(100)).await;

    let requests = server.requests.lock().unwrap();
    assert!(requests.iter().all(|r| r.path != "/ingestion/insert_block"));
    assert!(
        requests
            .iter()
            .any(|r| r.path == "/ingestion/insert_process")
    );
    assert!(requests.iter().any(|r| r.path == "/ingestion/insert_stream"
        && body_contains(&r.body, &log_stream.stream_id().to_string())));
}

fn body_contains_any(requests: &[CapturedRequest], needle: &str) -> bool {
    requests.iter().any(|r| body_contains(&r.body, needle))
}
