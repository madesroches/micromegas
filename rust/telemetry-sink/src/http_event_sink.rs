use micromegas_telemetry::stream_info::StreamInfo;
use micromegas_telemetry::wire_format::encode_cbor;
use micromegas_tracing::{
    event::{EventSink, TracingBlock},
    flush_monitor::FlushMonitor,
    images::{ImageBlock, ImageStream},
    logs::{LogBlock, LogMetadata, LogStream},
    metrics::{MetricsBlock, MetricsStream},
    prelude::*,
    property_set::Property,
    spans::{ThreadBlock, ThreadStream},
};
use std::{
    cmp::max,
    collections::{HashMap, VecDeque},
    fmt,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicIsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_retry2::{RetryError, strategy::ExponentialBackoff};

use crate::request_decorator::RequestDecorator;
use crate::stream_block::StreamBlock;
use crate::stream_info::make_stream_info;

/// A retry strategy: an exponential backoff, capped to a fixed number of attempts.
type RetryStrategy = core::iter::Take<ExponentialBackoff>;

/// Error type for ingestion client operations.
/// Explicitly categorizes errors to control retry behavior.
///
/// Logging strategy: Transient errors (5xx, network) use `debug!` to avoid
/// polluting instrumented applications' logs with telemetry infrastructure noise.
/// Permanent errors (4xx) use `warn!` since they indicate a bug in the client.
#[derive(Clone, Debug)]
enum IngestionClientError {
    /// Transient error - should retry (network issues, 5xx responses)
    Transient(String),
    /// Permanent error - should NOT retry (4xx responses, malformed data)
    Permanent(String),
}

impl std::fmt::Display for IngestionClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IngestionClientError::Transient(msg) => write!(f, "transient error: {msg}"),
            IngestionClientError::Permanent(msg) => write!(f, "permanent error: {msg}"),
        }
    }
}

impl std::error::Error for IngestionClientError {}

impl IngestionClientError {
    fn into_retry(self) -> RetryError<Self> {
        match self {
            IngestionClientError::Transient(_) => RetryError::transient(self),
            IngestionClientError::Permanent(_) => RetryError::permanent(self),
        }
    }
}

/// Upload priority classes, drained strictly in this order (Metadata first,
/// Traces last) and used to decide which items are shed first under
/// backpressure. Mirrors the Unreal telemetry sink's `EUploadPriority`.
///
/// Also indexes [`HttpSinkConfig::retry_by_priority`]: index 0 is Metadata,
/// 1 is Logs, 2 is Metrics, 3 is Traces.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UploadPriority {
    /// `insert_process` / `insert_stream` — never dropped.
    Metadata = 0,
    /// Log blocks.
    Logs = 1,
    /// Metrics blocks.
    Metrics = 2,
    /// Thread and image blocks.
    Traces = 3,
}

const NUM_PRIORITIES: usize = 4;

/// How long `on_shutdown`/`Drop` wait for the worker's final drain before
/// giving up on a graceful shutdown.
const SHUTDOWN_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

fn record_dropped_metric(priority: UploadPriority) {
    match priority {
        UploadPriority::Metadata => {
            imetric!("telemetry_dropped_metadata", "count", 1);
        }
        UploadPriority::Logs => {
            imetric!("telemetry_dropped_logs", "count", 1);
        }
        UploadPriority::Metrics => {
            imetric!("telemetry_dropped_metrics", "count", 1);
        }
        UploadPriority::Traces => {
            imetric!("telemetry_dropped_traces", "count", 1);
        }
    }
}

enum Payload {
    Process(Arc<ProcessInfo>),
    Stream(Arc<StreamInfo>),
    Block {
        block: Arc<dyn StreamBlock + Send + Sync>,
        kind: &'static str,
    },
}

struct QueuedItem {
    priority: UploadPriority,
    /// Raw uncompressed queued size (`0` for metadata, `len_bytes()` for blocks).
    bytes: usize,
    payload: Payload,
}

/// A priority-ordered, byte-budgeted queue shared between the application
/// threads (enqueue side) and the dedicated upload worker thread (drain side).
///
/// The `notify` condvar is paired with the `queues` mutex: every state change
/// that the worker might be waiting on (a new item, a freed in-flight slot, or
/// shutdown) is signaled while holding (or right after briefly re-acquiring)
/// that same mutex. This is what prevents a lost wakeup: the worker always
/// re-checks its wake predicate under the lock immediately before waiting, so
/// a signal can never land in the gap between the check and the wait.
struct SharedQueue {
    queues: Mutex<[VecDeque<QueuedItem>; NUM_PRIORITIES]>,
    queue_bytes: AtomicIsize,
    queue_count: AtomicIsize,
    notify: Condvar,
    shutdown: AtomicBool,
    soft_bytes: usize,
    hard_bytes: usize,
}

impl SharedQueue {
    fn new(soft_bytes: usize, hard_bytes: usize) -> Self {
        Self {
            queues: Mutex::new([
                VecDeque::new(),
                VecDeque::new(),
                VecDeque::new(),
                VecDeque::new(),
            ]),
            queue_bytes: AtomicIsize::new(0),
            queue_count: AtomicIsize::new(0),
            notify: Condvar::new(),
            shutdown: AtomicBool::new(false),
            soft_bytes,
            hard_bytes: hard_bytes.max(soft_bytes),
        }
    }

    /// Enqueues `item`, applying the graded byte-budget drop policy. Returns
    /// `false` if the item was dropped instead of queued.
    fn try_push(&self, item: QueuedItem) -> bool {
        let current = self.queue_bytes.load(Ordering::Relaxed).max(0) as usize;
        let should_drop = match item.priority {
            UploadPriority::Metadata => false,
            UploadPriority::Traces => current >= self.soft_bytes,
            UploadPriority::Logs | UploadPriority::Metrics => current >= self.hard_bytes,
        };
        if should_drop {
            record_dropped_metric(item.priority);
            return false;
        }
        // The shutdown check and the push must happen under the same lock
        // that guards `pop_highest`'s drain loop: otherwise an enqueuer could
        // observe `shutdown == false`, get preempted, and push after the
        // worker's final drain has already run to completion and signaled
        // `shutdown_complete` — silently leaking the item into an abandoned
        // queue. Checking here, inside the critical section, makes this
        // mutually exclusive with the drain's emptiness check.
        let mut guard = self.queues.lock().unwrap();
        if self.shutdown.load(Ordering::SeqCst) {
            drop(guard);
            // The worker has committed to (or already finished) its final
            // drain and will never pop this item, so queuing it would leak
            // it forever. Reject and report instead, matching the old
            // mpsc-based sink's behavior of logging a post-shutdown send.
            error!(
                "dropping telemetry item enqueued after shutdown, priority={:?}",
                item.priority
            );
            record_dropped_metric(item.priority);
            return false;
        }
        self.queue_bytes
            .fetch_add(item.bytes as isize, Ordering::Relaxed);
        self.queue_count.fetch_add(1, Ordering::Relaxed);
        guard[item.priority as usize].push_back(item);
        self.notify.notify_all();
        true
    }

    /// Pops the highest-priority queued item (Metadata, then Logs, Metrics,
    /// Traces), if any.
    fn pop_highest(&self) -> Option<QueuedItem> {
        let mut guard = self.queues.lock().unwrap();
        for deque in guard.iter_mut() {
            if let Some(item) = deque.pop_front() {
                drop(guard);
                self.queue_bytes
                    .fetch_sub(item.bytes as isize, Ordering::Relaxed);
                self.queue_count.fetch_sub(1, Ordering::Relaxed);
                return Some(item);
            }
        }
        None
    }

    /// Wakes the worker. Used both for actual state changes (shutdown) and as
    /// a pure synchronization fence (a completed send freeing an in-flight
    /// slot) — see the struct-level doc comment for why acquiring `queues`
    /// here is required for correctness, not just a mutation guard.
    fn wake(&self) {
        let _guard = self.queues.lock().unwrap();
        self.notify.notify_all();
    }
}

/// Configuration for [`HttpEventSink`]'s transport: how much to buffer, how
/// aggressively to shed load under backpressure, how much concurrency to
/// allow, and how hard to retry each priority class.
///
/// The byte caps sit far above what a healthy co-located ingestion service
/// (e.g. a monolith) will ever accumulate, so a normal run drops nothing;
/// they only bite during a real outage.
pub struct HttpSinkConfig {
    /// Soft cap, in bytes: once the queue holds at least this many bytes,
    /// new `Traces` items (thread and image blocks) are dropped. Default 128 MiB.
    pub max_queue_bytes: usize,
    /// Hard cap, in bytes: once the queue holds at least this many bytes,
    /// new `Logs`/`Metrics` items are dropped too. Clamped to be at least
    /// `max_queue_bytes`. `Metadata` (process/stream) is never dropped.
    /// Default 256 MiB.
    pub hard_queue_bytes: usize,
    /// Maximum number of `insert_*` HTTP requests in flight at once. Set to
    /// `1` to restore strictly serial sends. Default 3.
    pub max_in_flight_requests: usize,
    /// Per-request timeout (covers connect + send + receive for one attempt).
    ///
    /// This is a deliberate addition beyond Unreal parity: Unreal's
    /// per-priority retry window is a total retry *budget*, not a socket
    /// timeout (it never sets one either), so a single attempt against an
    /// ingestion service that accepts the TCP connection but never responds
    /// can hang indefinitely. Without a bound here, that hang is fatal at
    /// shutdown: `Drop for HttpEventSink` joins the worker thread, so a
    /// short-lived process would freeze on exit whenever ingestion is
    /// unresponsive (as opposed to merely offline, which fails fast with a
    /// connection error). Default 10 seconds.
    pub request_timeout: Duration,
    /// Retry strategy per [`UploadPriority`] (indexed by
    /// `UploadPriority as usize`).
    pub retry_by_priority: [RetryStrategy; NUM_PRIORITIES],
}

impl HttpSinkConfig {
    /// Default soft byte cap: 128 MiB.
    pub const DEFAULT_MAX_QUEUE_BYTES: usize = 128 * 1024 * 1024;
    /// Default hard byte cap: 256 MiB.
    pub const DEFAULT_HARD_QUEUE_BYTES: usize = 256 * 1024 * 1024;
    /// Default in-flight request cap (Unreal parity).
    pub const DEFAULT_MAX_IN_FLIGHT_REQUESTS: usize = 3;
    /// Default per-request timeout.
    pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

    /// The default per-priority retry table: Metadata gets the most retries,
    /// Traces the fewest, mirroring the Unreal sink's
    /// `RetryCountByPriority` (`{10,5,2,1}`).
    pub fn default_retry_by_priority() -> [RetryStrategy; NUM_PRIORITIES] {
        [
            ExponentialBackoff::from_millis(10).take(10), // Metadata
            ExponentialBackoff::from_millis(10).take(5),  // Logs
            ExponentialBackoff::from_millis(10).take(2),  // Metrics
            ExponentialBackoff::from_millis(10).take(1),  // Traces
        ]
    }
}

impl Default for HttpSinkConfig {
    fn default() -> Self {
        Self {
            max_queue_bytes: Self::DEFAULT_MAX_QUEUE_BYTES,
            hard_queue_bytes: Self::DEFAULT_HARD_QUEUE_BYTES,
            max_in_flight_requests: Self::DEFAULT_MAX_IN_FLIGHT_REQUESTS,
            request_timeout: Self::DEFAULT_REQUEST_TIMEOUT,
            retry_by_priority: Self::default_retry_by_priority(),
        }
    }
}

/// State shared by every send spawned by the worker loop: the HTTP client,
/// the current process info (set synchronously as soon as the `Startup` item
/// is dequeued, independently of whether the send itself succeeds), and the
/// per-priority retry table.
struct WorkerShared {
    client: reqwest::Client,
    addr: String,
    process_info: Mutex<Option<Arc<ProcessInfo>>>,
    decorator: Arc<dyn RequestDecorator>,
    retry_by_priority: [RetryStrategy; NUM_PRIORITIES],
}

/// The subset of [`HttpSinkConfig`] needed by the worker thread, grouped to
/// keep `thread_proc`/`run`'s parameter lists manageable.
struct WorkerConfig {
    max_in_flight_requests: usize,
    request_timeout: Duration,
    retry_by_priority: [RetryStrategy; NUM_PRIORITIES],
}

pub struct HttpEventSink {
    thread: Option<std::thread::JoinHandle<()>>,
    queue: Arc<SharedQueue>,
    pending_stream_meta: Mutex<HashMap<uuid::Uuid, Arc<StreamInfo>>>,
    shutdown_complete: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

impl Drop for HttpEventSink {
    fn drop(&mut self) {
        // Bounded like `on_shutdown`: if the worker is stuck (e.g. mid-retry
        // against an ingestion service that accepts connections but never
        // responds), we must not block process exit on `handle.join()`
        // indefinitely. Abandon the thread instead: the OS reclaims it when
        // the process exits, or if it eventually finishes on its own.
        if !self.signal_shutdown_and_wait() {
            eprintln!(
                "Warning: telemetry thread did not shut down within the timeout, abandoning it"
            );
            return;
        }

        if let Some(handle) = self.thread.take()
            && let Err(e) = handle.join()
        {
            // Don't panic on join failure, just log it
            eprintln!("Warning: telemetry thread join failed: {:?}", e);
        }
    }
}

impl HttpEventSink {
    /// Creates a new `HttpEventSink`.
    ///
    /// This function spawns a new thread that handles sending telemetry data
    /// to the specified HTTP server. Sends run on a dedicated tokio runtime,
    /// with up to `config.max_in_flight_requests` requests in flight
    /// concurrently, drained strictly in priority order (Metadata, Logs,
    /// Metrics, Traces).
    ///
    /// # Arguments
    ///
    /// * `addr_server` - The address of the HTTP server to send data to.
    /// * `config` - Transport configuration: byte budgets, concurrency, retries.
    /// * `make_decorator` - A closure that returns a `RequestDecorator` for modifying HTTP requests.
    pub fn new(
        addr_server: &str,
        config: HttpSinkConfig,
        make_decorator: Box<dyn FnOnce() -> Arc<dyn RequestDecorator> + Send>,
    ) -> Self {
        let addr = addr_server.to_owned();
        let queue = Arc::new(SharedQueue::new(
            config.max_queue_bytes,
            config.hard_queue_bytes,
        ));
        let thread_queue = queue.clone();
        let worker_config = WorkerConfig {
            max_in_flight_requests: config.max_in_flight_requests,
            request_timeout: config.request_timeout,
            retry_by_priority: config.retry_by_priority,
        };
        let shutdown_complete = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let thread_shutdown_complete = shutdown_complete.clone();
        Self {
            thread: Some(std::thread::spawn(move || {
                Self::thread_proc(
                    addr,
                    thread_queue,
                    worker_config,
                    make_decorator,
                    thread_shutdown_complete,
                );
            })),
            queue,
            pending_stream_meta: Mutex::new(HashMap::new()),
            shutdown_complete,
        }
    }

    /// Signals shutdown and waits (bounded) for the worker's final drain to
    /// complete. Returns `false` if the wait timed out — the worker may
    /// still be stuck mid-send, and the thread should not be joined
    /// unconditionally in that case.
    fn signal_shutdown_and_wait(&self) -> bool {
        self.queue.shutdown.store(true, Ordering::SeqCst);
        self.queue.wake();

        let (lock, cvar) = &*self.shutdown_complete;
        let completed = lock.lock().unwrap();
        let (completed, result) = cvar
            .wait_timeout_while(completed, SHUTDOWN_WAIT_TIMEOUT, |&mut c| !c)
            .unwrap();
        drop(completed);
        !result.timed_out()
    }

    fn flush_pending_stream_meta(&self, stream_id: uuid::Uuid) {
        if let Some(stream_info) = self.pending_stream_meta.lock().unwrap().remove(&stream_id) {
            self.queue.try_push(QueuedItem {
                priority: UploadPriority::Metadata,
                bytes: 0,
                payload: Payload::Stream(stream_info),
            });
        }
    }

    #[span_fn]
    async fn push_process(
        client: &reqwest::Client,
        root_path: &str,
        process_info: Arc<ProcessInfo>,
        retry_strategy: RetryStrategy,
        decorator: &dyn RequestDecorator,
    ) -> Result<(), IngestionClientError> {
        debug!("sending process {process_info:?}");
        let url = format!("{root_path}/ingestion/insert_process");
        let body: bytes::Bytes = encode_cbor(&*process_info)
            .map_err(|e| IngestionClientError::Permanent(format!("encoding process: {e}")))?
            .into();
        tokio_retry2::Retry::spawn(retry_strategy, || async {
            let mut request = client.post(&url).body(body.clone()).build().map_err(|e| {
                IngestionClientError::Permanent(format!("building request: {e}")).into_retry()
            })?;

            if let Err(e) = decorator.decorate(&mut request).await {
                debug!("request decorator: {e:?}");
                return Err(
                    IngestionClientError::Transient(format!("decorating request: {e}"))
                        .into_retry(),
                );
            }

            let response = client.execute(request).await.map_err(|e| {
                IngestionClientError::Transient(format!("network error: {e}")).into_retry()
            })?;

            let status = response.status();
            match status.as_u16() {
                200..=299 => Ok(()),
                400..=499 => {
                    let body = response.text().await.unwrap_or_default();
                    warn!("insert_process client error ({status}): {body}");
                    Err(IngestionClientError::Permanent(body).into_retry())
                }
                500..=599 => {
                    let body = response.text().await.unwrap_or_default();
                    debug!("insert_process server error ({status}): {body}");
                    Err(IngestionClientError::Transient(format!("{status}: {body}")).into_retry())
                }
                _ => {
                    let body = response.text().await.unwrap_or_default();
                    warn!("insert_process unexpected status ({status}): {body}");
                    Err(IngestionClientError::Permanent(format!("{status}: {body}")).into_retry())
                }
            }
        })
        .await
    }

    #[span_fn]
    async fn push_stream(
        client: &reqwest::Client,
        root_path: &str,
        stream_info: Arc<StreamInfo>,
        retry_strategy: RetryStrategy,
        decorator: &dyn RequestDecorator,
    ) -> Result<(), IngestionClientError> {
        let url = format!("{root_path}/ingestion/insert_stream");
        let body: bytes::Bytes = encode_cbor(&*stream_info)
            .map_err(|e| IngestionClientError::Permanent(format!("encoding stream: {e}")))?
            .into();
        tokio_retry2::Retry::spawn(retry_strategy, || async {
            let mut request = client.post(&url).body(body.clone()).build().map_err(|e| {
                IngestionClientError::Permanent(format!("building request: {e}")).into_retry()
            })?;

            if let Err(e) = decorator.decorate(&mut request).await {
                debug!("request decorator: {e:?}");
                return Err(
                    IngestionClientError::Transient(format!("decorating request: {e}"))
                        .into_retry(),
                );
            }

            let response = client.execute(request).await.map_err(|e| {
                IngestionClientError::Transient(format!("network error: {e}")).into_retry()
            })?;

            let status = response.status();
            match status.as_u16() {
                200..=299 => Ok(()),
                400..=499 => {
                    let body = response.text().await.unwrap_or_default();
                    warn!("insert_stream client error ({status}): {body}");
                    Err(IngestionClientError::Permanent(body).into_retry())
                }
                500..=599 => {
                    let body = response.text().await.unwrap_or_default();
                    debug!("insert_stream server error ({status}): {body}");
                    Err(IngestionClientError::Transient(format!("{status}: {body}")).into_retry())
                }
                _ => {
                    let body = response.text().await.unwrap_or_default();
                    warn!("insert_stream unexpected status ({status}): {body}");
                    Err(IngestionClientError::Permanent(format!("{status}: {body}")).into_retry())
                }
            }
        })
        .await
    }

    #[span_fn]
    async fn push_block(
        client: &reqwest::Client,
        root_path: &str,
        buffer: &dyn StreamBlock,
        retry_strategy: RetryStrategy,
        decorator: &dyn RequestDecorator,
        process_info: &ProcessInfo,
    ) -> Result<(), IngestionClientError> {
        trace!("push_block");
        let encoded_block: bytes::Bytes = buffer
            .encode_bin(process_info)
            .map_err(|e| IngestionClientError::Permanent(format!("encoding block: {e}")))?
            .into();

        let url = format!("{root_path}/ingestion/insert_block");

        tokio_retry2::Retry::spawn(retry_strategy, || async {
            let mut request = client
                .post(&url)
                .body(encoded_block.clone())
                .build()
                .map_err(|e| {
                    IngestionClientError::Permanent(format!("building request: {e}")).into_retry()
                })?;

            if let Err(e) = decorator.decorate(&mut request).await {
                debug!("request decorator: {e:?}");
                return Err(
                    IngestionClientError::Transient(format!("decorating request: {e}"))
                        .into_retry(),
                );
            }

            trace!("push_block: executing request");

            let response = client.execute(request).await.map_err(|e| {
                IngestionClientError::Transient(format!("network error: {e}")).into_retry()
            })?;

            let status = response.status();
            match status.as_u16() {
                200..=299 => Ok(()),
                400..=499 => {
                    let body = response.text().await.unwrap_or_default();
                    warn!("insert_block client error ({status}): {body}");
                    Err(IngestionClientError::Permanent(body).into_retry())
                }
                500..=599 => {
                    let body = response.text().await.unwrap_or_default();
                    debug!("insert_block server error ({status}): {body}");
                    Err(IngestionClientError::Transient(format!("{status}: {body}")).into_retry())
                }
                _ => {
                    let body = response.text().await.unwrap_or_default();
                    warn!("insert_block unexpected status ({status}): {body}");
                    Err(IngestionClientError::Permanent(format!("{status}: {body}")).into_retry())
                }
            }
        })
        .await
    }

    /// Dispatches one dequeued item onto the tokio runtime. `permit`, if
    /// present, is held for the duration of the send and gates concurrency;
    /// during the final shutdown drain it is `None` so all remaining items go
    /// out at once. `Payload::Process` sets the shared `process_info`
    /// synchronously here (before spawning), matching the pre-existing
    /// guarantee that blocks can rely on it being set as soon as `Startup` is
    /// dequeued, independently of whether the send itself succeeds.
    fn spawn_item(
        shared: &Arc<WorkerShared>,
        queue: &Arc<SharedQueue>,
        item: QueuedItem,
        join_set: &mut tokio::task::JoinSet<()>,
        permit: Option<OwnedSemaphorePermit>,
    ) {
        if let Payload::Process(ref info) = item.payload {
            *shared.process_info.lock().unwrap() = Some(info.clone());
        }
        let shared = shared.clone();
        let queue = queue.clone();
        let retry_strategy = shared.retry_by_priority[item.priority as usize].clone();
        join_set.spawn(async move {
            match item.payload {
                Payload::Process(info) => {
                    if let Err(e) = Self::push_process(
                        &shared.client,
                        &shared.addr,
                        info,
                        retry_strategy,
                        shared.decorator.as_ref(),
                    )
                    .await
                    {
                        error!("error sending process: {e}");
                    }
                }
                Payload::Stream(stream_info) => {
                    if let Err(e) = Self::push_stream(
                        &shared.client,
                        &shared.addr,
                        stream_info,
                        retry_strategy,
                        shared.decorator.as_ref(),
                    )
                    .await
                    {
                        error!("error sending stream: {e}");
                    }
                }
                Payload::Block { block, kind } => {
                    let maybe_process_info = shared.process_info.lock().unwrap().clone();
                    if let Some(process_info) = maybe_process_info {
                        if let Err(e) = Self::push_block(
                            &shared.client,
                            &shared.addr,
                            block.as_ref(),
                            retry_strategy,
                            shared.decorator.as_ref(),
                            &process_info,
                        )
                        .await
                        {
                            error!("error sending {kind}: {e}");
                        }
                    } else {
                        error!("trying to send blocks before Startup message");
                    }
                }
            }
            drop(permit);
            queue.wake();
        });
    }

    async fn run(
        addr: String,
        queue: Arc<SharedQueue>,
        config: WorkerConfig,
        make_decorator: Box<dyn FnOnce() -> Arc<dyn RequestDecorator> + Send>,
        shutdown_complete: Arc<(Mutex<bool>, std::sync::Condvar)>,
    ) {
        let client = match reqwest::Client::builder()
            .pool_idle_timeout(Some(core::time::Duration::from_secs(2)))
            .timeout(config.request_timeout)
            .build()
        {
            Ok(client) => client,
            Err(e) => {
                error!("Error creating http client: {e:?}");
                return;
            }
        };
        // eagerly connect, a new process message is sure to follow if it's not already in queue
        if let Some(process_id) = micromegas_tracing::dispatch::process_id() {
            let cpu_tracing_enabled =
                micromegas_tracing::dispatch::cpu_tracing_enabled().unwrap_or(false);
            info!("process_id={process_id}, cpu_tracing_enabled={cpu_tracing_enabled}");
        }
        let shared = Arc::new(WorkerShared {
            client,
            addr,
            process_info: Mutex::new(None),
            decorator: make_decorator(),
            retry_by_priority: config.retry_by_priority,
        });
        let semaphore = Arc::new(Semaphore::new(config.max_in_flight_requests.max(1)));
        let mut join_set: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();
        let flusher = FlushMonitor::default();

        loop {
            if queue.shutdown.load(Ordering::SeqCst) {
                break;
            }

            loop {
                let permit = match Arc::clone(&semaphore).try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => break,
                };
                match queue.pop_highest() {
                    Some(item) => {
                        Self::spawn_item(&shared, &queue, item, &mut join_set, Some(permit))
                    }
                    None => {
                        drop(permit);
                        break;
                    }
                }
            }
            // Reap finished sends so the JoinSet doesn't grow unbounded.
            while let Some(result) = join_set.try_join_next() {
                if let Err(e) = result {
                    error!("telemetry send task panicked: {e}");
                }
            }

            // clamp to zero as the original code did: time_to_flush_seconds() returns i64
            // and goes negative when a flush is overdue; a negative value cast to
            // Duration::from_secs(u64) would wrap to ~u64::MAX and never wake.
            let timeout = Duration::from_secs(max(0, flusher.time_to_flush_seconds()) as u64);
            {
                let guard = queue.queues.lock().unwrap();
                let empty = guard.iter().all(VecDeque::is_empty);
                let permit_available = semaphore.available_permits() > 0;
                if !queue.shutdown.load(Ordering::SeqCst) && (empty || !permit_available) {
                    let _ = queue.notify.wait_timeout(guard, timeout);
                }
            }
            flusher.tick();
        }

        debug!("received shutdown signal, flushing remaining data");
        // Final drain: submit everything left, bypassing the concurrency gate
        // so shutdown doesn't wait on `max_in_flight_requests` round trips.
        let mut drained = 0;
        while let Some(item) = queue.pop_highest() {
            drained += 1;
            Self::spawn_item(&shared, &queue, item, &mut join_set, None);
        }
        while let Some(result) = join_set.join_next().await {
            if let Err(e) = result {
                error!("telemetry send task panicked: {e}");
            }
        }
        debug!("telemetry thread shutdown complete, drained {drained} remaining items");

        let (lock, cvar) = &*shutdown_complete;
        let mut completed = lock.lock().unwrap();
        *completed = true;
        cvar.notify_all();
    }

    fn thread_proc(
        addr: String,
        queue: Arc<SharedQueue>,
        config: WorkerConfig,
        make_decorator: Box<dyn FnOnce() -> Arc<dyn RequestDecorator> + Send>,
        shutdown_complete: Arc<(Mutex<bool>, std::sync::Condvar)>,
    ) {
        // TODO: add runtime as configuration option (or create one only if global don't exist)
        let tokio_runtime = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                error!("Failed to create tokio runtime for telemetry: {e}");
                return;
            }
        };
        tokio_runtime.block_on(Self::run(
            addr,
            queue,
            config,
            make_decorator,
            shutdown_complete,
        ));
    }
}

impl EventSink for HttpEventSink {
    fn on_startup(&self, process_info: Arc<ProcessInfo>) {
        self.queue.try_push(QueuedItem {
            priority: UploadPriority::Metadata,
            bytes: 0,
            payload: Payload::Process(process_info),
        });
    }

    fn on_shutdown(&self) {
        // Signals shutdown, wakes the worker, and waits (bounded) for its
        // final drain. If it times out, the caller (and the eventual `Drop`)
        // still won't block indefinitely — see `signal_shutdown_and_wait`.
        self.signal_shutdown_and_wait();
    }

    fn on_log_enabled(&self, _metadata: &LogMetadata) -> bool {
        // If all previous filter succeeds this sink always agrees
        true
    }

    fn on_log(
        &self,
        _metadata: &LogMetadata,
        _properties: &[Property],
        _time: i64,
        _args: fmt::Arguments<'_>,
    ) {
    }

    fn on_init_log_stream(&self, log_stream: &LogStream) {
        self.pending_stream_meta.lock().unwrap().insert(
            log_stream.stream_id(),
            Arc::new(make_stream_info(log_stream)),
        );
    }

    fn on_process_log_block(&self, log_block: Arc<LogBlock>) {
        self.flush_pending_stream_meta(log_block.stream_id);
        let bytes = log_block.len_bytes();
        self.queue.try_push(QueuedItem {
            priority: UploadPriority::Logs,
            bytes,
            payload: Payload::Block {
                block: log_block,
                kind: "log block",
            },
        });
    }

    fn on_init_metrics_stream(&self, metrics_stream: &MetricsStream) {
        self.pending_stream_meta.lock().unwrap().insert(
            metrics_stream.stream_id(),
            Arc::new(make_stream_info(metrics_stream)),
        );
    }

    fn on_process_metrics_block(&self, metrics_block: Arc<MetricsBlock>) {
        self.flush_pending_stream_meta(metrics_block.stream_id);
        let bytes = metrics_block.len_bytes();
        self.queue.try_push(QueuedItem {
            priority: UploadPriority::Metrics,
            bytes,
            payload: Payload::Block {
                block: metrics_block,
                kind: "metrics block",
            },
        });
    }

    fn on_init_thread_stream(&self, thread_stream: &ThreadStream) {
        self.pending_stream_meta.lock().unwrap().insert(
            thread_stream.stream_id(),
            Arc::new(make_stream_info(thread_stream)),
        );
    }

    fn on_process_thread_block(&self, thread_block: Arc<ThreadBlock>) {
        self.flush_pending_stream_meta(thread_block.stream_id);
        let bytes = thread_block.len_bytes();
        self.queue.try_push(QueuedItem {
            priority: UploadPriority::Traces,
            bytes,
            payload: Payload::Block {
                block: thread_block,
                kind: "thread block",
            },
        });
    }

    fn on_init_image_stream(&self, stream: &ImageStream) {
        self.pending_stream_meta
            .lock()
            .unwrap()
            .insert(stream.stream_id(), Arc::new(make_stream_info(stream)));
    }

    fn on_process_image_block(&self, block: Arc<ImageBlock>) {
        self.flush_pending_stream_meta(block.stream_id);
        let bytes = block.len_bytes();
        self.queue.try_push(QueuedItem {
            priority: UploadPriority::Traces,
            bytes,
            payload: Payload::Block {
                block,
                kind: "image block",
            },
        });
    }

    fn is_busy(&self) -> bool {
        let count = self.queue.queue_count.load(Ordering::Relaxed);
        debug_assert!(count >= 0, "queue_count went negative: {count}");
        count > 0
    }
}
