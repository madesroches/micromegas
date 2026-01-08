use micromegas_telemetry::stream_info::StreamInfo;
use micromegas_telemetry::wire_format::encode_cbor;
use micromegas_tracing::{
    event::EventSink,
    flush_monitor::FlushMonitor,
    logs::{LogBlock, LogMetadata, LogStream},
    metrics::{MetricsBlock, MetricsStream},
    prelude::*,
    property_set::Property,
    spans::{ThreadBlock, ThreadStream},
};
use std::{
    cmp::max,
    fmt,
    sync::{Arc, Mutex},
};
use std::{
    sync::atomic::{AtomicIsize, Ordering},
    time::Duration,
};
use tokio_retry2::RetryError;

use crate::request_decorator::RequestDecorator;
use crate::stream_block::StreamBlock;
use crate::stream_info::make_stream_info;

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

#[derive(Debug)]
enum SinkEvent {
    Startup(Arc<ProcessInfo>),
    InitStream(Arc<StreamInfo>),
    ProcessLogBlock(Arc<LogBlock>),
    ProcessMetricsBlock(Arc<MetricsBlock>),
    ProcessThreadBlock(Arc<ThreadBlock>),
    Shutdown,
}

pub struct HttpEventSink {
    thread: Option<std::thread::JoinHandle<()>>,
    // TODO: simplify this?
    sender: Mutex<Option<std::sync::mpsc::Sender<SinkEvent>>>,
    queue_size: Arc<AtomicIsize>,
    shutdown_complete: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

impl Drop for HttpEventSink {
    fn drop(&mut self) {
        // Send shutdown signal to thread
        {
            let sender_guard = self.sender.lock().unwrap();
            if let Some(sender) = sender_guard.as_ref() {
                let _ = sender.send(SinkEvent::Shutdown);
            }
        }

        // Now wait for the thread to finish
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
    /// to the specified HTTP server.
    ///
    /// # Arguments
    ///
    /// * `addr_server` - The address of the HTTP server to send data to.
    /// * `max_queue_size` - The maximum number of events to queue before dropping them.
    /// * `metadata_retry` - The retry strategy for sending metadata (process info, stream info).
    /// * `blocks_retry` - The retry strategy for sending blocks (log, metrics, thread).
    /// * `make_decorator` - A closure that returns a `RequestDecorator` for modifying HTTP requests.
    pub fn new(
        addr_server: &str,
        max_queue_size: isize,
        metadata_retry: core::iter::Take<tokio_retry2::strategy::ExponentialBackoff>,
        blocks_retry: core::iter::Take<tokio_retry2::strategy::ExponentialBackoff>,
        make_decorator: Box<dyn FnOnce() -> Arc<dyn RequestDecorator> + Send>,
    ) -> Self {
        let addr = addr_server.to_owned();
        let (sender, receiver) = std::sync::mpsc::channel::<SinkEvent>();
        let queue_size = Arc::new(AtomicIsize::new(0));
        let thread_queue_size = queue_size.clone();
        let shutdown_complete = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let thread_shutdown_complete = shutdown_complete.clone();
        Self {
            thread: Some(std::thread::spawn(move || {
                Self::thread_proc(
                    addr,
                    receiver,
                    thread_queue_size,
                    max_queue_size,
                    metadata_retry,
                    blocks_retry,
                    make_decorator,
                    thread_shutdown_complete,
                );
            })),
            sender: Mutex::new(Some(sender)),
            queue_size,
            shutdown_complete,
        }
    }

    fn send(&self, event: SinkEvent) {
        let guard = self.sender.lock().unwrap();
        if let Some(sender) = guard.as_ref() {
            self.queue_size.fetch_add(1, Ordering::Relaxed);
            if let Err(e) = sender.send(event) {
                self.queue_size.fetch_sub(1, Ordering::Relaxed);
                error!("{}", e);
            }
        }
    }

    #[span_fn]
    async fn push_process(
        client: &mut reqwest::Client,
        root_path: &str,
        process_info: Arc<ProcessInfo>,
        retry_strategy: core::iter::Take<tokio_retry2::strategy::ExponentialBackoff>,
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
        client: &mut reqwest::Client,
        root_path: &str,
        stream_info: Arc<StreamInfo>,
        retry_strategy: core::iter::Take<tokio_retry2::strategy::ExponentialBackoff>,
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
    #[expect(clippy::too_many_arguments)]
    async fn push_block(
        client: &mut reqwest::Client,
        root_path: &str,
        buffer: &dyn StreamBlock,
        current_queue_size: &AtomicIsize,
        max_queue_size: isize,
        retry_strategy: core::iter::Take<tokio_retry2::strategy::ExponentialBackoff>,
        decorator: &dyn RequestDecorator,
        process_info: &ProcessInfo,
    ) -> Result<(), IngestionClientError> {
        trace!("push_block");
        if current_queue_size.load(Ordering::Relaxed) >= max_queue_size {
            // could be better to have a budget for each block type
            // this way thread data would not starve the other streams
            debug!("dropping data, queue over max_queue_size");
            return Ok(());
        }
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

    #[expect(clippy::too_many_arguments)]
    async fn handle_sink_event(
        message: SinkEvent,
        client: &mut reqwest::Client,
        addr: &str,
        opt_process_info: &mut Option<Arc<ProcessInfo>>,
        queue_size: &Arc<AtomicIsize>,
        max_queue_size: isize,
        metadata_retry: core::iter::Take<tokio_retry2::strategy::ExponentialBackoff>,
        blocks_retry: core::iter::Take<tokio_retry2::strategy::ExponentialBackoff>,
        decorator: &dyn RequestDecorator,
    ) {
        match message {
            SinkEvent::Shutdown => {
                // This should not happen in this function, but handle it gracefully
            }
            SinkEvent::Startup(process_info) => {
                *opt_process_info = Some(process_info.clone());
                if let Err(e) =
                    Self::push_process(client, addr, process_info, metadata_retry, decorator).await
                {
                    error!("error sending process: {e}");
                }
            }
            SinkEvent::InitStream(stream_info) => {
                if let Err(e) =
                    Self::push_stream(client, addr, stream_info, metadata_retry, decorator).await
                {
                    error!("error sending stream: {e}");
                }
            }
            SinkEvent::ProcessLogBlock(buffer) => {
                if let Some(process_info) = opt_process_info {
                    if let Err(e) = Self::push_block(
                        client,
                        addr,
                        &*buffer,
                        queue_size,
                        max_queue_size,
                        blocks_retry,
                        decorator,
                        process_info,
                    )
                    .await
                    {
                        error!("error sending log block: {e}");
                    }
                } else {
                    error!("trying to send blocks before Startup message");
                }
            }
            SinkEvent::ProcessMetricsBlock(buffer) => {
                if let Some(process_info) = opt_process_info {
                    if let Err(e) = Self::push_block(
                        client,
                        addr,
                        &*buffer,
                        queue_size,
                        max_queue_size,
                        blocks_retry,
                        decorator,
                        process_info,
                    )
                    .await
                    {
                        error!("error sending metrics block: {e}");
                    }
                } else {
                    error!("trying to send blocks before Startup message");
                }
            }
            SinkEvent::ProcessThreadBlock(buffer) => {
                if let Some(process_info) = opt_process_info {
                    if let Err(e) = Self::push_block(
                        client,
                        addr,
                        &*buffer,
                        queue_size,
                        max_queue_size,
                        blocks_retry,
                        decorator,
                        process_info,
                    )
                    .await
                    {
                        error!("error sending thread block: {e}");
                    }
                } else {
                    error!("trying to send blocks before Startup message");
                }
            }
        }
    }

    #[expect(clippy::too_many_arguments)]
    async fn thread_proc_impl(
        addr: String,
        receiver: std::sync::mpsc::Receiver<SinkEvent>,
        queue_size: Arc<AtomicIsize>,
        max_queue_size: isize,
        metadata_retry: core::iter::Take<tokio_retry2::strategy::ExponentialBackoff>,
        blocks_retry: core::iter::Take<tokio_retry2::strategy::ExponentialBackoff>,
        decorator: &dyn RequestDecorator,
        shutdown_complete: Arc<(Mutex<bool>, std::sync::Condvar)>,
    ) {
        let mut opt_process_info = None;
        let mut client = match reqwest::Client::builder()
            .pool_idle_timeout(Some(core::time::Duration::from_secs(2)))
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
        let flusher = FlushMonitor::default();
        loop {
            let timeout = max(0, flusher.time_to_flush_seconds());
            match receiver.recv_timeout(Duration::from_secs(timeout as u64)) {
                Ok(message) => {
                    queue_size.fetch_sub(1, Ordering::Relaxed);
                    match message {
                        SinkEvent::Shutdown => {
                            debug!("received shutdown signal, flushing remaining data");
                            // Process any remaining messages in the queue before shutting down
                            let mut count = 0;
                            while let Ok(remaining_message) = receiver.try_recv() {
                                count += 1;
                                match remaining_message {
                                    SinkEvent::Shutdown => break, // Don't process multiple shutdowns
                                    remaining_msg => {
                                        // Process the remaining message using the same logic as the main loop
                                        Self::handle_sink_event(
                                            remaining_msg,
                                            &mut client,
                                            &addr,
                                            &mut opt_process_info,
                                            &queue_size,
                                            max_queue_size,
                                            metadata_retry.clone(),
                                            blocks_retry.clone(),
                                            decorator,
                                        )
                                        .await;
                                    }
                                }
                            }
                            debug!(
                                "telemetry thread shutdown complete, processed {} remaining messages",
                                count
                            );
                            // Signal that shutdown is complete
                            let (lock, cvar) = &*shutdown_complete;
                            let mut completed = lock.lock().unwrap();
                            *completed = true;
                            cvar.notify_all();
                            return;
                        }
                        other_message => {
                            Self::handle_sink_event(
                                other_message,
                                &mut client,
                                &addr,
                                &mut opt_process_info,
                                &queue_size,
                                max_queue_size,
                                metadata_retry.clone(),
                                blocks_retry.clone(),
                                decorator,
                            )
                            .await;
                        }
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    flusher.tick();
                }
                Err(_e) => {
                    // can only fail when the sending half is disconnected
                    // println!("Error in telemetry thread: {}", e);
                    return;
                }
            }
        }
    }

    #[allow(clippy::needless_pass_by_value,// we don't want to leave the receiver in the calling thread
	    clippy::too_many_arguments
    )]
    fn thread_proc(
        addr: String,
        receiver: std::sync::mpsc::Receiver<SinkEvent>,
        queue_size: Arc<AtomicIsize>,
        max_queue_size: isize,
        metadata_retry: core::iter::Take<tokio_retry2::strategy::ExponentialBackoff>,
        blocks_retry: core::iter::Take<tokio_retry2::strategy::ExponentialBackoff>,
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
        let decorator = make_decorator();
        tokio_runtime.block_on(Self::thread_proc_impl(
            addr,
            receiver,
            queue_size,
            max_queue_size,
            metadata_retry,
            blocks_retry,
            decorator.as_ref(),
            shutdown_complete,
        ));
    }
}

impl EventSink for HttpEventSink {
    fn on_startup(&self, process_info: Arc<ProcessInfo>) {
        self.send(SinkEvent::Startup(process_info));
    }

    fn on_shutdown(&self) {
        // Send shutdown event to trigger flushing of remaining data
        self.send(SinkEvent::Shutdown);

        // Wait for the background thread to signal that shutdown is complete
        let (lock, cvar) = &*self.shutdown_complete;
        let completed = lock.lock().unwrap();
        let timeout = std::time::Duration::from_secs(5);
        let _result = cvar.wait_timeout_while(completed, timeout, |&mut c| !c);
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
        self.send(SinkEvent::InitStream(Arc::new(make_stream_info(
            log_stream,
        ))));
    }

    fn on_process_log_block(&self, log_block: Arc<LogBlock>) {
        self.send(SinkEvent::ProcessLogBlock(log_block));
    }

    fn on_init_metrics_stream(&self, metrics_stream: &MetricsStream) {
        self.send(SinkEvent::InitStream(Arc::new(make_stream_info(
            metrics_stream,
        ))));
    }

    fn on_process_metrics_block(&self, metrics_block: Arc<MetricsBlock>) {
        self.send(SinkEvent::ProcessMetricsBlock(metrics_block));
    }

    fn on_init_thread_stream(&self, thread_stream: &ThreadStream) {
        self.send(SinkEvent::InitStream(Arc::new(make_stream_info(
            thread_stream,
        ))));
    }

    fn on_process_thread_block(&self, thread_block: Arc<ThreadBlock>) {
        self.send(SinkEvent::ProcessThreadBlock(thread_block));
    }

    fn is_busy(&self) -> bool {
        self.queue_size.load(Ordering::Relaxed) > 0
    }
}
