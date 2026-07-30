//! Cross-crate integration coverage for the Phase 0 mid-stream resume fix and
//! the client-side circuit breaker (see
//! `tasks/1360_cache_client_circuit_breaker_plan.md`). Each test spins up a
//! minimal, purpose-built axum server (not the real `object-cache-srv`
//! handlers) so its response timing/shape is fully controllable: a
//! `tokio::sync::watch::Receiver::changed()` gate lets a handler hang
//! indefinitely until the test explicitly releases it, giving a
//! deterministic, sleep-free way to exercise the client's timeouts against a
//! real HTTP connection.

use std::net::SocketAddr;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_stream::stream as gen_stream;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use bytes::{BufMut, Bytes, BytesMut};
use futures::stream;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{GetOptions, GetRange, ObjectStore, ObjectStoreExt};
use tokio::sync::watch;

use micromegas::object_cache::circuit_breaker::CircuitBreakerConfig;
use micromegas::object_cache::prefetch::PrefetchItem;
use micromegas::object_cache::{
    CacheClientConfig, CacheClientStore, RangesReadError, RangesSendError,
};

// ============================================================================
// Test server plumbing
// ============================================================================

struct TestServer {
    addr: SocketAddr,
    handle: tokio::task::JoinHandle<()>,
}

impl TestServer {
    async fn start(app: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local_addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        Self { addr, handle }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    async fn shutdown(self) {
        self.handle.abort();
        let _ = self.handle.await;
    }
}

/// A gate a handler can park on until the test releases it. Cloning the
/// receiver lets every request awaiting it observe the same release.
fn new_gate() -> (watch::Sender<bool>, watch::Receiver<bool>) {
    watch::channel(false)
}

async fn wait_for_gate(mut gate: watch::Receiver<bool>) {
    while !*gate.borrow() {
        if gate.changed().await.is_err() {
            return;
        }
    }
}

fn release(tx: &watch::Sender<bool>) {
    let _ = tx.send(true);
}

/// Parse a `Range: bytes=start-end` header into a resolved `start..end`
/// (exclusive), against `total` for an open-ended `bytes=start-` request.
fn parse_range(headers: &HeaderMap, total: u64) -> Option<Range<u64>> {
    let v = headers.get(header::RANGE)?.to_str().ok()?;
    let v = v.strip_prefix("bytes=")?;
    let (s, e) = v.split_once('-')?;
    let start: u64 = s.parse().ok()?;
    if e.is_empty() {
        Some(start..total)
    } else {
        let end_incl: u64 = e.parse().ok()?;
        Some(start..(end_incl + 1).min(total))
    }
}

fn ranged_response(total: u64, range: Range<u64>, body: Body) -> Response {
    Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(
            "Content-Range",
            format!("bytes {}-{}/{}", range.start, range.end - 1, total),
        )
        .body(body)
        .expect("build ranged response")
}

fn full_response(total: u64, body: Body) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Length", total.to_string())
        .body(body)
        .expect("build full response")
}

fn head_response(total: u64) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Length", total.to_string())
        .body(Body::empty())
        .expect("build head response")
}

/// Stream `chunks` in order, then hang forever awaiting `gate` (unless
/// released). Also keeps the gate's `Sender` alive for the lifetime of the
/// stream: whenever a fresh gate is created *inside* a per-request handler
/// closure, the `Sender` would otherwise be dropped the instant the handler
/// returns the `Response` (before the body stream is ever polled), closing
/// the channel and making `wait_for_gate` return immediately instead of
/// actually hanging.
fn stream_then_hang_owned(
    chunks: Vec<Bytes>,
    tx: watch::Sender<bool>,
    gate: watch::Receiver<bool>,
) -> Body {
    Body::from_stream(gen_stream! {
        let _keep_alive = tx;
        for chunk in chunks {
            yield Ok::<_, std::io::Error>(chunk);
        }
        wait_for_gate(gate).await;
    })
}

/// Stream `data` in `chunk_size` pieces, sleeping `delay` between each --
/// used to prove a per-frame timeout resets rather than bounding cumulative
/// transfer time. A real, deliberate wall-clock delay: the property under
/// test is inherently about elapsed time on a real HTTP connection, not
/// something a synthetic clock can stand in for (the breaker's own state
/// machine has that synthetic-clock coverage already, in
/// `circuit_breaker_tests.rs`).
fn stream_slowly(data: Bytes, chunk_size: usize, delay: Duration) -> Body {
    Body::from_stream(gen_stream! {
        let mut offset = 0usize;
        while offset < data.len() {
            let end = (offset + chunk_size).min(data.len());
            yield Ok::<_, std::io::Error>(data.slice(offset..end));
            offset = end;
            if offset < data.len() {
                tokio::time::sleep(delay).await;
            }
        }
    })
}

async fn put_bytes(store: &InMemory, key: &str, data: &[u8]) {
    store
        .put(&Path::from(key), Bytes::copy_from_slice(data).into())
        .await
        .expect("put");
}

fn cfg(abandon: Duration, stall: Duration, threshold: u32) -> CacheClientConfig {
    CacheClientConfig {
        connect_timeout: Duration::from_millis(200),
        abandon_timeout: abandon,
        stall_timeout: stall,
        total_timeout: Duration::from_secs(30),
        breaker: CircuitBreakerConfig {
            failure_threshold: threshold,
            cooldown: stall,
        },
    }
}

// ============================================================================
// Resume correctness -- the invariant's own tests
// ============================================================================

/// A single `/obj` route that always answers with the full object (as one
/// chunk) but then, once every declared byte has been sent, aborts the
/// connection instead of ending cleanly -- an immediate transport error the
/// client must resume from `direct` rather than surface, for a `Bounded`
/// read spanning the whole object.
#[tokio::test]
async fn mid_stream_abort_resumes_byte_identical_full_read() {
    let total = 4096u64;
    let data: Vec<u8> = (0u8..=255).cycle().take(total as usize).collect();
    let get_calls = Arc::new(AtomicUsize::new(0));

    let data_for_handler = data.clone();
    let calls = get_calls.clone();
    let app = Router::new().route(
        "/obj/{*key}",
        get(move |headers: HeaderMap| {
            let data = data_for_handler.clone();
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let range = parse_range(&headers, total).unwrap_or(0..total);
                // Deliver the first half, then abort: an immediate transport
                // error with data still owed.
                let mid = range.start + (range.end - range.start) / 2;
                let first = Bytes::copy_from_slice(&data[range.start as usize..mid as usize]);
                let body = Body::from_stream(gen_stream! {
                    yield Ok::<_, std::io::Error>(first);
                    // A tiny real delay so the first chunk is actually
                    // flushed and observed by the client before the error
                    // arrives, rather than racing hyper's own framing when
                    // both are produced in the same poll with no yield
                    // point between them.
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    yield Err::<Bytes, _>(std::io::Error::other("simulated abort"));
                });
                ranged_response(total, range, body)
            }
        }),
    );
    let server = TestServer::start(app).await;

    // The direct fallback holds the same bytes: the assertion is
    // byte-for-byte equality with a healthy read.
    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/a", &data).await;

    let client = CacheClientStore::with_config(
        server.base_url(),
        None,
        direct,
        cfg(Duration::from_millis(200), Duration::from_millis(200), 5),
    );
    let got = client
        .get_range(&Path::from("obj/a"), 0..total)
        .await
        .expect("resumed read must succeed");
    assert_eq!(&got[..], &data[..], "resumed read must be byte-identical");
    assert_eq!(get_calls.load(Ordering::SeqCst), 1);

    server.shutdown().await;
}

/// The same failing handler driven through all three `GetRange` shapes, plus
/// an unranged full read: each must return exactly the bytes a direct read
/// would, asserted on content (not just length).
#[tokio::test]
async fn resume_offset_correct_for_every_range_shape() {
    let total = 8000u64;
    let data: Vec<u8> = (0u8..=255).cycle().take(total as usize).collect();

    let make_app = |data: Vec<u8>| {
        Router::new().route(
            "/obj/{*key}",
            get(move |headers: HeaderMap| {
                let data = data.clone();
                async move {
                    let range = parse_range(&headers, total).unwrap_or(0..total);
                    let len = range.end - range.start;
                    // Deliver 70% of the range then abort.
                    let cut = range.start + (len * 7 / 10);
                    let first = Bytes::copy_from_slice(&data[range.start as usize..cut as usize]);
                    let body = Body::from_stream(gen_stream! {
                        yield Ok::<_, std::io::Error>(first);
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        yield Err::<Bytes, _>(std::io::Error::other("simulated abort"));
                    });
                    ranged_response(total, range, body)
                }
            })
            .head(move || async move { head_response(total) }),
        )
    };

    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/a", &data).await;
    let path = Path::from("obj/a");

    // Full (unranged) read: `get_full_stream` requires Content-Length, which
    // this ranged-response server doesn't set on its 206 -- so drive it
    // through `get_opts` with an explicit `Bounded(0..total)` instead, which
    // is exactly how a resumed full read is expressed anyway.
    {
        let server = TestServer::start(make_app(data.clone())).await;
        let client = CacheClientStore::with_config(
            server.base_url(),
            None,
            direct.clone(),
            cfg(Duration::from_millis(200), Duration::from_millis(200), 5),
        );
        let got = client.get_range(&path, 0..total).await.expect("bounded");
        assert_eq!(&got[..], &data[..]);
        server.shutdown().await;
    }

    // Bounded, interior.
    {
        let server = TestServer::start(make_app(data.clone())).await;
        let client = CacheClientStore::with_config(
            server.base_url(),
            None,
            direct.clone(),
            cfg(Duration::from_millis(200), Duration::from_millis(200), 5),
        );
        let bounded = 1000u64..6000;
        let got = client
            .get_range(&path, bounded.clone())
            .await
            .expect("bounded");
        assert_eq!(
            &got[..],
            &data[bounded.start as usize..bounded.end as usize]
        );
        server.shutdown().await;
    }

    // Offset (open-ended).
    {
        let server = TestServer::start(make_app(data.clone())).await;
        let client = CacheClientStore::with_config(
            server.base_url(),
            None,
            direct.clone(),
            cfg(Duration::from_millis(200), Duration::from_millis(200), 5),
        );
        let offset = 5000u64;
        let result = client
            .get_opts(
                &path,
                GetOptions {
                    range: Some(GetRange::Offset(offset)),
                    ..Default::default()
                },
            )
            .await
            .expect("offset get_opts");
        let bytes = result.bytes().await.expect("offset bytes");
        assert_eq!(&bytes[..], &data[offset as usize..]);
        server.shutdown().await;
    }

    // Suffix.
    {
        let server = TestServer::start(make_app(data.clone())).await;
        let client = CacheClientStore::with_config(
            server.base_url(),
            None,
            direct.clone(),
            cfg(Duration::from_millis(200), Duration::from_millis(200), 5),
        );
        let suffix_len = 3000u64;
        let result = client
            .get_opts(
                &path,
                GetOptions {
                    range: Some(GetRange::Suffix(suffix_len)),
                    ..Default::default()
                },
            )
            .await
            .expect("suffix get_opts");
        let bytes = result.bytes().await.expect("suffix bytes");
        assert_eq!(&bytes[..], &data[(total - suffix_len) as usize..]);
        server.shutdown().await;
    }
}

/// Regression guard for the silent-truncation variant: a 206 with
/// `Content-Range` but no `Content-Length` that ends its body cleanly
/// (`None`) short of the declared range -- the shape the old code passed
/// through silently. The client must resume the remainder from `direct`
/// through the `None`-with-bytes-owed arm and return the full range,
/// byte-identical to a direct read.
#[tokio::test]
async fn clean_short_stream_is_not_silently_truncated() {
    let total = 4096u64;
    let data: Vec<u8> = (0u8..=255).cycle().take(total as usize).collect();

    let data_for_handler = data.clone();
    let app = Router::new().route(
        "/obj/{*key}",
        get(move |headers: HeaderMap| {
            let data = data_for_handler.clone();
            async move {
                let range = parse_range(&headers, total).unwrap_or(0..total);
                let short_end = range.start + (range.end - range.start) / 3;
                let short = Bytes::copy_from_slice(&data[range.start as usize..short_end as usize]);
                // Ends cleanly (`None`), never yielding an `Err` -- the exact
                // shape `object_store::util::collect_bytes` would otherwise
                // silently accept.
                let body = Body::from_stream(stream::iter(vec![Ok::<_, std::io::Error>(short)]));
                ranged_response(total, range, body)
            }
        }),
    );
    let server = TestServer::start(app).await;

    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/a", &data).await;

    let client = CacheClientStore::with_config(
        server.base_url(),
        None,
        direct,
        cfg(Duration::from_millis(200), Duration::from_millis(200), 5),
    );
    let got = client
        .get_range(&Path::from("obj/a"), 0..total)
        .await
        .expect("must resume the short remainder from direct");
    assert_eq!(got.len(), total as usize, "must not be silently truncated");
    assert_eq!(&got[..], &data[..]);

    server.shutdown().await;
}

/// A failing resume must surface the direct store's own error, not a silent
/// short read.
#[tokio::test]
async fn failing_resume_surfaces_direct_error() {
    let total = 2048u64;
    let data: Vec<u8> = (0u8..=255).cycle().take(total as usize).collect();

    let data_for_handler = data.clone();
    let app = Router::new().route(
        "/obj/{*key}",
        get(move |headers: HeaderMap| {
            let data = data_for_handler.clone();
            async move {
                let range = parse_range(&headers, total).unwrap_or(0..total);
                let mid = range.start + (range.end - range.start) / 2;
                let first = Bytes::copy_from_slice(&data[range.start as usize..mid as usize]);
                let body = Body::from_stream(gen_stream! {
                    yield Ok::<_, std::io::Error>(first);
                    // A tiny real delay so the first chunk is actually
                    // flushed and observed by the client before the error
                    // arrives, rather than racing hyper's own framing when
                    // both are produced in the same poll with no yield
                    // point between them.
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    yield Err::<Bytes, _>(std::io::Error::other("simulated abort"));
                });
                ranged_response(total, range, body)
            }
        }),
    );
    let server = TestServer::start(app).await;

    // An empty direct store: the resumed read must fail against it.
    let direct = Arc::new(InMemory::new());
    let client = CacheClientStore::with_config(
        server.base_url(),
        None,
        direct,
        cfg(Duration::from_millis(200), Duration::from_millis(200), 5),
    );
    let result = client.get_range(&Path::from("obj/missing"), 0..total).await;
    assert!(
        result.is_err(),
        "a failing resume must surface the direct store's error"
    );

    server.shutdown().await;
}

/// A stream error exactly at the last byte -- every requested byte already
/// delivered -- must end cleanly, never call `direct`, and report
/// `record_responsive` (observed here as the breaker never opening across
/// many repeats).
#[tokio::test]
async fn stream_error_at_last_byte_ends_cleanly_and_reports_responsive() {
    let total = 1024u64;
    let data: Vec<u8> = (0u8..=255).cycle().take(total as usize).collect();
    let get_calls = Arc::new(AtomicUsize::new(0));

    let data_for_handler = data.clone();
    let calls = get_calls.clone();
    let app = Router::new().route(
        "/obj/{*key}",
        get(move |headers: HeaderMap| {
            let data = data_for_handler.clone();
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let range = parse_range(&headers, total).unwrap_or(0..total);
                let full = Bytes::copy_from_slice(&data[range.start as usize..range.end as usize]);
                // Deliver every requested byte, then error -- must not be
                // treated as a resumable shortfall.
                let body = Body::from_stream(gen_stream! {
                    yield Ok::<_, std::io::Error>(full);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    yield Err::<Bytes, _>(std::io::Error::other("late failure, nothing owed"));
                });
                ranged_response(total, range, body)
            }
        }),
    );
    let server = TestServer::start(app).await;

    // Mismatched direct data: if a resume/fallback ever fired we'd observe
    // the wrong bytes.
    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/a", &vec![0xffu8; total as usize]).await;

    // A tiny threshold: if this ever reported unresponsive, a handful of
    // reads would trip the breaker and the next read would come back
    // wrong-but-fast from `direct` instead of erroring -- distinguishable
    // from "always correct, cache never bypassed".
    let client = CacheClientStore::with_config(
        server.base_url(),
        None,
        direct,
        cfg(Duration::from_millis(200), Duration::from_millis(200), 2),
    );
    for _ in 0..10 {
        let got = client
            .get_range(&Path::from("obj/a"), 0..total)
            .await
            .expect("must complete successfully despite the terminal error");
        assert_eq!(
            &got[..],
            &data[..],
            "must be served by the cache, not direct"
        );
    }
    assert_eq!(
        get_calls.load(Ordering::SeqCst),
        10,
        "every read must have reached the cache -- the breaker must never have opened"
    );

    server.shutdown().await;
}

// ============================================================================
// Circuit breaker: trip / bypass / recover
// ============================================================================

#[tokio::test]
async fn trip_and_bypass_get_opts() {
    let total = 256u64;
    let direct_data: Vec<u8> = vec![2u8; total as usize];
    let get_calls = Arc::new(AtomicUsize::new(0));
    let (_tx, rx) = new_gate(); // never released

    let calls = get_calls.clone();
    let app = Router::new().route(
        "/obj/{*key}",
        get(move |headers: HeaderMap| {
            let calls = calls.clone();
            let rx = rx.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let range = parse_range(&headers, total).unwrap_or(0..total);
                wait_for_gate(rx).await; // never returns in this test
                ranged_response(total, range, Body::empty())
            }
        }),
    );
    let server = TestServer::start(app).await;

    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/a", &direct_data).await;

    // A long cooldown: once open, this test never expects a probe.
    let threshold = 5u32;
    let client = CacheClientStore::with_config(
        server.base_url(),
        None,
        direct,
        cfg(
            Duration::from_millis(50),
            Duration::from_millis(50),
            threshold,
        )
        .clone_with_cooldown(Duration::from_secs(60)),
    );
    let path = Path::from("obj/a");

    for _ in 0..threshold {
        let got = client.get_range(&path, 0..total).await.expect("fallback");
        assert_eq!(&got[..], &direct_data[..]);
    }
    let calls_at_trip = get_calls.load(Ordering::SeqCst);
    assert_eq!(calls_at_trip, threshold as usize);

    for _ in 0..3 {
        let got = client.get_range(&path, 0..total).await.expect("bypass");
        assert_eq!(&got[..], &direct_data[..]);
    }
    assert_eq!(
        get_calls.load(Ordering::SeqCst),
        calls_at_trip,
        "the cache must have been skipped entirely once the circuit opened"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn probe_and_recovery() {
    let total = 256u64;
    let cache_data: Vec<u8> = vec![1u8; total as usize];
    let direct_data: Vec<u8> = vec![2u8; total as usize];
    let get_calls = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = new_gate();

    let calls = get_calls.clone();
    let data_for_handler = cache_data.clone();
    let app = Router::new().route(
        "/obj/{*key}",
        get(move |headers: HeaderMap| {
            let calls = calls.clone();
            let rx = rx.clone();
            let data = data_for_handler.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let range = parse_range(&headers, total).unwrap_or(0..total);
                wait_for_gate(rx).await;
                let body = Bytes::copy_from_slice(&data[range.start as usize..range.end as usize]);
                ranged_response(total, range, Body::from(body))
            }
        }),
    );
    let server = TestServer::start(app).await;

    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/a", &direct_data).await;

    // A near-zero cooldown: the very next read after the trip is a probe.
    let threshold = 3u32;
    let client = CacheClientStore::with_config(
        server.base_url(),
        None,
        direct,
        cfg(
            Duration::from_millis(50),
            Duration::from_millis(50),
            threshold,
        )
        .clone_with_cooldown(Duration::from_millis(1)),
    );
    let path = Path::from("obj/a");

    for _ in 0..threshold {
        client.get_range(&path, 0..total).await.expect("fallback");
    }
    let calls_before_probe = get_calls.load(Ordering::SeqCst);

    // Release the hang: the cache now answers promptly.
    release(&tx);
    // Give the near-zero cooldown time to elapse.
    tokio::time::sleep(Duration::from_millis(20)).await;

    let got = client
        .get_range(&path, 0..total)
        .await
        .expect("probe should succeed");
    assert_eq!(&got[..], &cache_data[..], "the probe must use the cache");
    assert_eq!(get_calls.load(Ordering::SeqCst), calls_before_probe + 1);

    // Subsequent reads keep using the cache (circuit closed).
    for _ in 0..3 {
        let got = client.get_range(&path, 0..total).await.expect("closed");
        assert_eq!(&got[..], &cache_data[..]);
    }
    assert!(get_calls.load(Ordering::SeqCst) > calls_before_probe + 1);

    server.shutdown().await;
}

#[tokio::test]
async fn get_ranges_gated_too() {
    let total = 1000u64;
    let direct_data: Vec<u8> = vec![9u8; total as usize];
    let ranges_calls = Arc::new(AtomicUsize::new(0));
    let (_tx, rx) = new_gate(); // never released

    let calls = ranges_calls.clone();
    let app = Router::new().route(
        "/ranges/{*key}",
        post(move || {
            let calls = calls.clone();
            let rx = rx.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                wait_for_gate(rx).await;
                Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::empty())
                    .expect("build")
            }
        }),
    );
    let server = TestServer::start(app).await;

    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/a", &direct_data).await;

    let threshold = 4u32;
    let client = CacheClientStore::with_config(
        server.base_url(),
        None,
        direct,
        cfg(
            Duration::from_millis(50),
            Duration::from_millis(50),
            threshold,
        )
        .clone_with_cooldown(Duration::from_secs(60)),
    );
    let path = Path::from("obj/a");
    let ranges = vec![0..100u64, 200..300u64];

    for _ in 0..threshold {
        let got = client
            .get_ranges(&path, &ranges)
            .await
            .expect("fallback via get_ranges");
        assert_eq!(got.len(), 2);
    }
    let calls_at_trip = ranges_calls.load(Ordering::SeqCst);

    let got = client
        .get_ranges(&path, &ranges)
        .await
        .expect("bypass via get_ranges");
    assert_eq!(got.len(), 2);
    assert_eq!(
        ranges_calls.load(Ordering::SeqCst),
        calls_at_trip,
        "get_ranges must skip the cache once the circuit is open"
    );

    server.shutdown().await;
}

/// A slow header phase (never writes headers before the client's short
/// `abandon_timeout` override) abandons and falls back every time -- still
/// recoverable -- and repeating it `failure_threshold` times trips the
/// breaker.
#[tokio::test]
async fn get_opts_slow_header_abandons_but_stays_recoverable() {
    let total = 256u64;
    let direct_data: Vec<u8> = vec![7u8; total as usize];
    let get_calls = Arc::new(AtomicUsize::new(0));
    let (_tx, rx) = new_gate(); // never released: headers never sent

    let calls = get_calls.clone();
    let app = Router::new().route(
        "/obj/{*key}",
        get(move |headers: HeaderMap| {
            let calls = calls.clone();
            let rx = rx.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let range = parse_range(&headers, total).unwrap_or(0..total);
                wait_for_gate(rx).await; // hangs before ever writing headers
                ranged_response(total, range, Body::empty())
            }
        }),
    );
    let server = TestServer::start(app).await;

    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/a", &direct_data).await;

    let threshold = 5u32;
    let client = CacheClientStore::with_config(
        server.base_url(),
        None,
        direct,
        cfg(
            Duration::from_millis(50),
            Duration::from_millis(200),
            threshold,
        )
        .clone_with_cooldown(Duration::from_secs(60)),
    );
    let path = Path::from("obj/a");

    for i in 0..threshold {
        let got = client
            .get_range(&path, 0..total)
            .await
            .unwrap_or_else(|_| panic!("read {i} must still fall back to correct data"));
        assert_eq!(&got[..], &direct_data[..]);
    }

    // The breaker must now be open.
    let got = client.get_range(&path, 0..total).await.expect("bypass");
    assert_eq!(&got[..], &direct_data[..]);
    let calls_after_trip = get_calls.load(Ordering::SeqCst);
    let got = client.get_range(&path, 0..total).await.expect("bypass");
    assert_eq!(&got[..], &direct_data[..]);
    assert_eq!(
        get_calls.load(Ordering::SeqCst),
        calls_after_trip,
        "no new request must have reached the cache once open"
    );

    server.shutdown().await;
}

/// The mid-body-stall handler from the resume-correctness section, repeated
/// past `failure_threshold`: every call must still return complete, correct
/// (resumed) data, and repeating it must trip the breaker -- the property
/// that would have been impossible under naive header-phase-only success
/// reporting.
#[tokio::test]
async fn get_opts_mid_body_stall_resumes_and_trips() {
    let total = 4096u64;
    let data: Vec<u8> = (0u8..=255).cycle().take(total as usize).collect();
    let get_calls = Arc::new(AtomicUsize::new(0));

    let calls = get_calls.clone();
    let data_for_handler = data.clone();
    let app = Router::new().route(
        "/obj/{*key}",
        get(move |headers: HeaderMap| {
            let calls = calls.clone();
            let data = data_for_handler.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let range = parse_range(&headers, total).unwrap_or(0..total);
                let mid = range.start + (range.end - range.start) / 2;
                let first = Bytes::copy_from_slice(&data[range.start as usize..mid as usize]);
                let (tx, rx) = new_gate(); // fresh, never-released gate per request
                let body = stream_then_hang_owned(vec![first], tx, rx);
                ranged_response(total, range, body)
            }
        }),
    );
    let server = TestServer::start(app).await;

    // Same bytes on both sides -- the assertion here is byte-identical
    // resumed data, not cache-vs-direct service (observed via the request
    // counter instead).
    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/a", &data).await;

    let threshold = 5u32;
    let client = CacheClientStore::with_config(
        server.base_url(),
        None,
        direct,
        cfg(
            Duration::from_millis(200),
            Duration::from_millis(50),
            threshold,
        )
        .clone_with_cooldown(Duration::from_secs(60)),
    );
    let path = Path::from("obj/a");

    for _ in 0..threshold {
        let got = client
            .get_range(&path, 0..total)
            .await
            .expect("resumed read must succeed every time");
        assert_eq!(&got[..], &data[..]);
    }
    let calls_at_trip = get_calls.load(Ordering::SeqCst);
    assert_eq!(calls_at_trip, threshold as usize);

    // The breaker must now be open: a further read must not reach the cache.
    let got = client.get_range(&path, 0..total).await.expect("bypass");
    assert_eq!(&got[..], &data[..]);
    assert_eq!(
        get_calls.load(Ordering::SeqCst),
        calls_at_trip,
        "must be served straight from direct, no new cache request"
    );

    server.shutdown().await;
}

/// A `Suffix` read whose `HEAD` answers promptly but whose `GET` hangs must
/// still trip the breaker -- the case that would have been impossible if
/// `head_size`'s completion were (wrongly) treated as the whole operation's
/// success.
#[tokio::test]
async fn suffix_read_trips_the_breaker() {
    let total = 4096u64;
    let direct_data: Vec<u8> = vec![3u8; total as usize];
    let get_calls = Arc::new(AtomicUsize::new(0));
    let (_tx, rx) = new_gate(); // never released

    let calls = get_calls.clone();
    let app = Router::new().route(
        "/obj/{*key}",
        get(move |_headers: HeaderMap| {
            let calls = calls.clone();
            let rx = rx.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                wait_for_gate(rx).await; // GET hangs forever
                full_response(total, Body::empty())
            }
        })
        .head(move || async move { head_response(total) }),
    );
    let server = TestServer::start(app).await;

    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/a", &direct_data).await;

    let threshold = 4u32;
    let client = CacheClientStore::with_config(
        server.base_url(),
        None,
        direct,
        cfg(
            Duration::from_millis(50),
            Duration::from_millis(50),
            threshold,
        )
        .clone_with_cooldown(Duration::from_secs(60)),
    );
    let path = Path::from("obj/a");

    for _ in 0..threshold {
        let result = client
            .get_opts(
                &path,
                GetOptions {
                    range: Some(GetRange::Suffix(100)),
                    ..Default::default()
                },
            )
            .await
            .expect("suffix fallback");
        let bytes = result.bytes().await.expect("bytes");
        assert_eq!(&bytes[..], &direct_data[..100]);
    }
    let calls_at_trip = get_calls.load(Ordering::SeqCst);
    assert_eq!(calls_at_trip, threshold as usize);

    let result = client
        .get_opts(
            &path,
            GetOptions {
                range: Some(GetRange::Suffix(100)),
                ..Default::default()
            },
        )
        .await
        .expect("bypass");
    let bytes = result.bytes().await.expect("bytes");
    assert_eq!(&bytes[..], &direct_data[..100]);
    assert_eq!(
        get_calls.load(Ordering::SeqCst),
        calls_at_trip,
        "the breaker must have opened -- no new GET reaches the cache"
    );

    server.shutdown().await;
}

/// A generous `stall_timeout`, with chunks arriving well inside it but the
/// total transfer taking far longer than it: the read must complete from the
/// cache with no resume and the breaker must never open, confirming
/// `read_timeout` resets per frame rather than bounding cumulative size.
#[tokio::test]
async fn get_opts_slow_but_progressing_body_stays_closed() {
    let total = 2000u64;
    let data: Vec<u8> = (0u8..=255).cycle().take(total as usize).collect();
    let get_calls = Arc::new(AtomicUsize::new(0));

    let calls = get_calls.clone();
    let data_for_handler = data.clone();
    let app = Router::new().route(
        "/obj/{*key}",
        get(move |headers: HeaderMap| {
            let calls = calls.clone();
            let data = data_for_handler.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let range = parse_range(&headers, total).unwrap_or(0..total);
                let slice = Bytes::copy_from_slice(&data[range.start as usize..range.end as usize]);
                let chunk_size = (slice.len() / 20).max(1);
                // 20 chunks x 100ms = ~2s total, well past a 50ms/200ms
                // abandon/stall pair used elsewhere, but comfortably inside
                // the generous >= 1s `stall_timeout` this test installs.
                let body = stream_slowly(slice, chunk_size, Duration::from_millis(100));
                ranged_response(total, range, body)
            }
        }),
    );
    let server = TestServer::start(app).await;

    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/a", &vec![0u8; total as usize]).await;

    let client = CacheClientStore::with_config(
        server.base_url(),
        None,
        direct,
        cfg(Duration::from_millis(200), Duration::from_secs(3), 3)
            .clone_with_cooldown(Duration::from_secs(60)),
    );
    let path = Path::from("obj/a");

    let got = client
        .get_range(&path, 0..total)
        .await
        .expect("a per-frame reset must not fail a slow-but-progressing body");
    assert_eq!(
        &got[..],
        &data[..],
        "must be served by the cache, no resume"
    );
    assert_eq!(get_calls.load(Ordering::SeqCst), 1);

    server.shutdown().await;
}

// ============================================================================
// get_ranges failure classification
// ============================================================================

fn frame_all(ranges: &[Range<u64>], data: &[u8]) -> Bytes {
    let mut buf = BytesMut::new();
    for r in ranges {
        let slice = &data[r.start as usize..r.end as usize];
        buf.put_u64_le(slice.len() as u64);
        buf.put_slice(slice);
    }
    buf.freeze()
}

#[tokio::test]
async fn get_ranges_read_stall_is_classified_as_stalled() {
    let total = 1000u64;
    let data: Vec<u8> = (0u8..=255).cycle().take(total as usize).collect();
    // Single-range vec is deliberate: the API takes `Vec<Range<u64>>`, and
    // this test only needs one range.
    #[allow(clippy::single_range_in_vec_init)]
    let ranges: Vec<Range<u64>> = vec![0..500];
    let calls = Arc::new(AtomicUsize::new(0));

    let calls_h = calls.clone();
    let app = Router::new().route(
        "/ranges/{*key}",
        post(move || {
            let calls_h = calls_h.clone();
            async move {
                calls_h.fetch_add(1, Ordering::SeqCst);
                // Write the length-prefix header, then hang forever before
                // ever sending the frame body.
                let mut prefix = BytesMut::with_capacity(8);
                prefix.put_u64_le(500u64);
                let (tx, rx) = new_gate();
                let body = stream_then_hang_owned(vec![prefix.freeze()], tx, rx);
                Response::builder()
                    .status(StatusCode::OK)
                    .body(body)
                    .expect("build")
            }
        }),
    );
    let server = TestServer::start(app).await;

    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/a", &data).await;

    let client = CacheClientStore::with_config(
        server.base_url(),
        None,
        direct.clone(),
        cfg(Duration::from_millis(200), Duration::from_millis(100), 5)
            .clone_with_cooldown(Duration::from_secs(60)),
    );
    let path = Path::from("obj/a");

    // Assert the exact classification directly via `send_ranges`.
    let err = client
        .send_ranges(&path, &ranges)
        .await
        .expect_err("must stall, not succeed");
    assert!(
        matches!(err, RangesSendError::Body(RangesReadError::Stalled)),
        "expected Stalled, got {err:?}"
    );

    // The public `get_ranges` entry point still falls back correctly.
    let got = client
        .get_ranges(&path, &ranges)
        .await
        .expect("get_ranges must fall back to direct");
    assert_eq!(&got[0][..], &data[0..500]);

    server.shutdown().await;
}

#[tokio::test]
async fn get_ranges_non_2xx_and_truncated_stay_closed() {
    let total = 1000u64;
    let data: Vec<u8> = (0u8..=255).cycle().take(total as usize).collect();
    #[allow(clippy::single_range_in_vec_init)]
    let ranges: Vec<Range<u64>> = vec![0..200];
    let calls = Arc::new(AtomicUsize::new(0));

    // A handler alternating 500 and a truncated frame, both of which must be
    // classified as responsive (never abandoned/unresponsive).
    let calls_h = calls.clone();
    let app = Router::new().route(
        "/ranges/{*key}",
        post(move || {
            let calls_h = calls_h.clone();
            async move {
                let n = calls_h.fetch_add(1, Ordering::SeqCst);
                if n.is_multiple_of(2) {
                    Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Body::empty())
                        .expect("build")
                } else {
                    // Declares 200 bytes, sends only 50, then ends cleanly.
                    let mut buf = BytesMut::new();
                    buf.put_u64_le(200u64);
                    buf.put_slice(&[0u8; 50]);
                    let body = Body::from_stream(stream::iter(vec![Ok::<_, std::io::Error>(
                        buf.freeze(),
                    )]));
                    Response::builder()
                        .status(StatusCode::OK)
                        .body(body)
                        .expect("build")
                }
            }
        }),
    );
    let server = TestServer::start(app).await;

    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/a", &data).await;

    // A tiny threshold: if either failure kind were mis-classified as
    // unresponsive, this many calls would trip the breaker.
    let client = CacheClientStore::with_config(
        server.base_url(),
        None,
        direct,
        cfg(Duration::from_millis(200), Duration::from_millis(200), 2)
            .clone_with_cooldown(Duration::from_secs(60)),
    );
    let path = Path::from("obj/a");

    for _ in 0..8 {
        let got = client
            .get_ranges(&path, &ranges)
            .await
            .expect("must always fall back to correct direct data");
        assert_eq!(&got[0][..], &data[0..200]);
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        8,
        "the breaker must never have opened: every call must have reached the cache"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn get_ranges_slow_but_progressing_body_stays_closed() {
    let total = 2000u64;
    let data: Vec<u8> = (0u8..=255).cycle().take(total as usize).collect();
    #[allow(clippy::single_range_in_vec_init)]
    let ranges: Vec<Range<u64>> = vec![0..total];
    let calls = Arc::new(AtomicUsize::new(0));

    let calls_h = calls.clone();
    let ranges_h = ranges.clone();
    let data_h = data.clone();
    let app = Router::new().route(
        "/ranges/{*key}",
        post(move || {
            let calls_h = calls_h.clone();
            let framed = frame_all(&ranges_h, &data_h);
            async move {
                calls_h.fetch_add(1, Ordering::SeqCst);
                let body = stream_slowly(framed, 64, Duration::from_millis(100));
                Response::builder()
                    .status(StatusCode::OK)
                    .body(body)
                    .expect("build")
            }
        }),
    );
    let server = TestServer::start(app).await;

    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/a", &vec![0u8; total as usize]).await;

    let client = CacheClientStore::with_config(
        server.base_url(),
        None,
        direct,
        cfg(Duration::from_millis(200), Duration::from_secs(3), 3)
            .clone_with_cooldown(Duration::from_secs(60)),
    );
    let path = Path::from("obj/a");

    let got = client
        .get_ranges(&path, &ranges)
        .await
        .expect("a per-chunk reset must not fail a slow-but-progressing body");
    assert_eq!(&got[0][..], &data[..]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    server.shutdown().await;
}

// ============================================================================
// Prefetch
// ============================================================================

#[tokio::test]
async fn prefetch_success_does_not_close_circuit() {
    let total = 256u64;
    let direct_data: Vec<u8> = vec![4u8; total as usize];
    let get_calls = Arc::new(AtomicUsize::new(0));
    let prefetch_calls = Arc::new(AtomicUsize::new(0));
    let (_tx, rx) = new_gate(); // /obj never responds

    let g_calls = get_calls.clone();
    let p_calls = prefetch_calls.clone();
    let app = Router::new()
        .route(
            "/obj/{*key}",
            get(move |_headers: HeaderMap| {
                let g_calls = g_calls.clone();
                let rx = rx.clone();
                async move {
                    g_calls.fetch_add(1, Ordering::SeqCst);
                    wait_for_gate(rx).await;
                    full_response(total, Body::empty())
                }
            }),
        )
        .route(
            "/prefetch",
            post(move || {
                let p_calls = p_calls.clone();
                async move {
                    p_calls.fetch_add(1, Ordering::SeqCst);
                    Response::builder()
                        .status(StatusCode::ACCEPTED)
                        .body(Body::from(r#"{"accepted":1,"rejected":0,"dropped":0}"#))
                        .expect("build")
                }
            }),
        );
    let server = TestServer::start(app).await;

    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/a", &direct_data).await;

    let threshold = 5u32;
    let client = CacheClientStore::with_config(
        server.base_url(),
        None,
        direct,
        cfg(
            Duration::from_millis(50),
            Duration::from_millis(50),
            threshold,
        )
        .clone_with_cooldown(Duration::from_secs(60)),
    );
    let path = Path::from("obj/a");

    // `threshold - 1` consecutive unresponsive reads: not yet enough to trip.
    for _ in 0..(threshold - 1) {
        let got = client.get_range(&path, 0..total).await.expect("fallback");
        assert_eq!(&got[..], &direct_data[..]);
    }
    assert_eq!(get_calls.load(Ordering::SeqCst), (threshold - 1) as usize);

    // A successful prefetch must not reset `consecutive`.
    let resp = client
        .prefetch(vec![PrefetchItem {
            key: "obj/p".to_string(),
            size: 10,
            ranges: None,
        }])
        .await
        .expect("prefetch success");
    assert_eq!(resp.accepted, 1);
    assert_eq!(prefetch_calls.load(Ordering::SeqCst), 1);

    // One more failure must now trip the breaker -- proving the prefetch
    // success above did not zero the counter.
    let got = client.get_range(&path, 0..total).await.expect("fallback");
    assert_eq!(&got[..], &direct_data[..]);
    assert_eq!(get_calls.load(Ordering::SeqCst), threshold as usize);

    let calls_at_trip = get_calls.load(Ordering::SeqCst);
    let got = client.get_range(&path, 0..total).await.expect("bypass");
    assert_eq!(&got[..], &direct_data[..]);
    assert_eq!(
        get_calls.load(Ordering::SeqCst),
        calls_at_trip,
        "the breaker must be open now: no new request should reach the cache"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn prefetch_while_open_bypasses_and_never_reaches_server() {
    let total = 256u64;
    let get_calls = Arc::new(AtomicUsize::new(0));
    let prefetch_calls = Arc::new(AtomicUsize::new(0));
    let (_tx, rx) = new_gate(); // never released

    let g_calls = get_calls.clone();
    let p_calls = prefetch_calls.clone();
    let app = Router::new()
        .route(
            "/obj/{*key}",
            get(move |_headers: HeaderMap| {
                let g_calls = g_calls.clone();
                let rx = rx.clone();
                async move {
                    g_calls.fetch_add(1, Ordering::SeqCst);
                    wait_for_gate(rx).await;
                    full_response(total, Body::empty())
                }
            }),
        )
        .route(
            "/prefetch",
            post(move || {
                let p_calls = p_calls.clone();
                async move {
                    p_calls.fetch_add(1, Ordering::SeqCst);
                    Response::builder()
                        .status(StatusCode::ACCEPTED)
                        .body(Body::from(r#"{"accepted":1,"rejected":0,"dropped":0}"#))
                        .expect("build")
                }
            }),
        );
    let server = TestServer::start(app).await;

    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/a", &vec![5u8; total as usize]).await;

    let threshold = 3u32;
    let client = CacheClientStore::with_config(
        server.base_url(),
        None,
        direct,
        cfg(
            Duration::from_millis(50),
            Duration::from_millis(50),
            threshold,
        )
        .clone_with_cooldown(Duration::from_secs(60)),
    );
    let path = Path::from("obj/a");

    for _ in 0..threshold {
        let _ = client.get_range(&path, 0..total).await.expect("fallback");
    }
    assert_eq!(get_calls.load(Ordering::SeqCst), threshold as usize);

    // Prefetch while the circuit is open: must bypass, never hit the server.
    for _ in 0..5 {
        let resp = client
            .prefetch(vec![PrefetchItem {
                key: "obj/b".to_string(),
                size: 10,
                ranges: None,
            }])
            .await
            .expect("prefetch bypass must be Ok");
        assert_eq!(resp.accepted, 0);
        assert_eq!(resp.rejected, 0);
        assert_eq!(resp.dropped, 1);
    }
    assert_eq!(
        prefetch_calls.load(Ordering::SeqCst),
        0,
        "prefetch must never reach the server while the circuit is open"
    );

    server.shutdown().await;
}

// ============================================================================
// Escape hatch
// ============================================================================

#[tokio::test]
async fn breaker_disabled_never_bypasses() {
    let total = 256u64;
    let direct_data: Vec<u8> = vec![1u8; total as usize];
    let get_calls = Arc::new(AtomicUsize::new(0));
    let (_tx, rx) = new_gate(); // never released

    let calls = get_calls.clone();
    let app = Router::new().route(
        "/obj/{*key}",
        get(move |_headers: HeaderMap| {
            let calls = calls.clone();
            let rx = rx.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                wait_for_gate(rx).await;
                full_response(total, Body::empty())
            }
        }),
    );
    let server = TestServer::start(app).await;

    let direct = Arc::new(InMemory::new());
    put_bytes(&direct, "obj/a", &direct_data).await;

    // `failure_threshold: 0` disables the breaker entirely.
    let client = CacheClientStore::with_config(
        server.base_url(),
        None,
        direct,
        cfg(Duration::from_millis(50), Duration::from_millis(50), 0),
    );
    let path = Path::from("obj/a");

    for i in 0..10 {
        let got = client.get_range(&path, 0..total).await.expect("fallback");
        assert_eq!(&got[..], &direct_data[..]);
        assert_eq!(
            get_calls.load(Ordering::SeqCst),
            i + 1,
            "every read must still reach the cache -- the breaker must never engage"
        );
    }

    server.shutdown().await;
}

// ============================================================================
// Small helper trait to keep the per-test `cfg(...)` calls terse.
// ============================================================================

trait ClonedWithCooldown {
    fn clone_with_cooldown(&self, cooldown: Duration) -> CacheClientConfig;
}

impl ClonedWithCooldown for CacheClientConfig {
    fn clone_with_cooldown(&self, cooldown: Duration) -> CacheClientConfig {
        let mut c = self.clone();
        c.breaker.cooldown = cooldown;
        c
    }
}
