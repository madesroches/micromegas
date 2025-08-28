use anyhow::{Context, Result};
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

use crate::request_decorator::RequestDecorator;
use crate::stream_block::StreamBlock;
use crate::stream_info::make_stream_info;

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
        eprintln!("HttpEventSink::drop() called");
        // Send shutdown signal to thread
        {
            let sender_guard = self.sender.lock().unwrap();
            if let Some(sender) = sender_guard.as_ref() {
                eprintln!("HttpEventSink::drop() sending shutdown event");
                let _ = sender.send(SinkEvent::Shutdown);
            }
        }

        // Now wait for the thread to finish
        eprintln!("HttpEventSink::drop() waiting for thread to join");
        if let Some(handle) = self.thread.take()
            && let Err(e) = handle.join()
        {
            // Don't panic on join failure, just log it
            eprintln!("Warning: telemetry thread join failed: {:?}", e);
        }
        eprintln!("HttpEventSink::drop() complete");
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
    ) -> Result<()> {
        debug!("sending process {process_info:?}");
        let url = format!("{root_path}/ingestion/insert_process");
        tokio_retry2::Retry::spawn(retry_strategy, || async {
            let body = encode_cbor(&*process_info)?;
            let mut request = client
                .post(&url)
                .body(body)
                .build()
                .with_context(|| "building request")?;

            if let Err(e) = decorator
                .decorate(&mut request)
                .await
                .with_context(|| "decorating request")
            {
                warn!("request decorator: {e:?}");
                return Err(e.into());
            }
            let result = client
                .execute(request)
                .await
                .with_context(|| "executing request");
            if let Err(e) = &result {
                debug!("insert_process error: {e:?}");
            }

            // Implicit error conversion. Result type needs to be `Result<T,RetryError<E>>` and not `Result<T,E>`, so using `?` or `map_transient_err` are the best ways to do it.
            Ok(result?)
        })
        .await?;
        Ok(())
    }

    #[span_fn]
    async fn push_stream(
        client: &mut reqwest::Client,
        root_path: &str,
        stream_info: Arc<StreamInfo>,
        retry_strategy: core::iter::Take<tokio_retry2::strategy::ExponentialBackoff>,
        decorator: &dyn RequestDecorator,
    ) -> Result<()> {
        let url = format!("{root_path}/ingestion/insert_stream");
        tokio_retry2::Retry::spawn(retry_strategy, || async {
            let body = encode_cbor(&*stream_info)?;
            let mut request = client
                .post(&url)
                .body(body)
                .build()
                .with_context(|| "building request")?;
            if let Err(e) = decorator.decorate(&mut request).await {
                warn!("request decorator: {e:?}");
                return Err(e.into());
            }
            let result = client
                .execute(request)
                .await
                .with_context(|| "executing request");
            if let Err(e) = &result {
                debug!("insert_stream error: {e}");
            }
            Ok(result?)
        })
        .await?;
        Ok(())
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
    ) -> Result<()> {
        trace!("push_block");
        if current_queue_size.load(Ordering::Relaxed) >= max_queue_size {
            // could be better to have a budget for each block type
            // this way thread data would not starve the other streams
            debug!("dropping data, queue over max_queue_size");
            return Ok(());
        }
        let encoded_block: bytes::Bytes = buffer.encode_bin(process_info)?.into();

        let url = format!("{root_path}/ingestion/insert_block");

        if let Err(err) = tokio_retry2::Retry::spawn(retry_strategy, || async {
            let mut request = client
                .post(&url)
                .body(encoded_block.clone())
                .build()
                .with_context(|| "building request")?;

            if let Err(e) = decorator
                .decorate(&mut request)
                .await
                .with_context(|| "decorating request")
            {
                debug!("request decorator: {e:?}");
                return Err(e.into());
            }

            trace!("push_block: executing request");

            client
                .execute(request)
                .await
                .with_context(|| "executing request")
                .map_err(Into::into)
        })
        .await
        {
            warn!("failed to push block: {err}");
        }

        Ok(())
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
    ) -> Result<()> {
        match message {
            SinkEvent::Shutdown => {
                // This should not happen in this function, but handle it gracefully
                return Ok(());
            }
            SinkEvent::Startup(process_info) => {
                *opt_process_info = Some(process_info.clone());
                if let Err(e) =
                    Self::push_process(client, addr, process_info, metadata_retry, decorator).await
                {
                    error!("error sending process: {e:?}");
                }
            }
            SinkEvent::InitStream(stream_info) => {
                if let Err(e) =
                    Self::push_stream(client, addr, stream_info, metadata_retry, decorator).await
                {
                    error!("error sending stream: {e:?}");
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
                        error!("error sending log block: {e:?}");
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
                        error!("error sending metrics block: {e:?}");
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
                        error!("error sending thread block: {e:?}");
                    }
                } else {
                    error!("trying to send blocks before Startup message");
                }
            }
        }
        Ok(())
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
        let client_res = reqwest::Client::builder()
            .pool_idle_timeout(Some(core::time::Duration::from_secs(2)))
            .build();
        if let Err(e) = client_res {
            error!("Error creating http client: {e:?}");
            return;
        }
        let mut client = client_res.unwrap();
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
                Ok(message) => match message {
                    SinkEvent::Shutdown => {
                        eprintln!(
                            "HttpEventSink thread: received shutdown signal, flushing remaining data"
                        );
                        debug!("received shutdown signal, flushing remaining data");
                        // Process any remaining messages in the queue before shutting down
                        let mut count = 0;
                        while let Ok(remaining_message) = receiver.try_recv() {
                            count += 1;
                            match remaining_message {
                                SinkEvent::Shutdown => break, // Don't process multiple shutdowns
                                remaining_msg => {
                                    // Process the remaining message using the same logic as the main loop
                                    if let Err(e) = Self::handle_sink_event(
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
                                    .await
                                    {
                                        error!(
                                            "error processing remaining message during shutdown: {e:?}"
                                        );
                                    }
                                }
                            }
                        }
                        eprintln!(
                            "HttpEventSink thread: processed {} remaining messages, signaling shutdown complete",
                            count
                        );
                        debug!("telemetry thread shutdown complete");
                        // Signal that shutdown is complete
                        let (lock, cvar) = &*shutdown_complete;
                        let mut completed = lock.lock().unwrap();
                        *completed = true;
                        cvar.notify_all();
                        eprintln!("HttpEventSink thread: shutdown signal sent, exiting");
                        return;
                    }
                    other_message => {
                        if let Err(e) = Self::handle_sink_event(
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
                        .await
                        {
                            error!("error handling sink event: {e:?}");
                        }
                    }
                },
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    flusher.tick();
                }
                Err(_e) => {
                    // can only fail when the sending half is disconnected
                    // println!("Error in telemetry thread: {}", e);
                    return;
                }
            }
            queue_size.fetch_sub(1, Ordering::Relaxed);
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
        let tokio_runtime = tokio::runtime::Runtime::new().unwrap();
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
        eprintln!("HttpEventSink::on_shutdown() called - sending shutdown event");
        // Send shutdown event to trigger flushing of remaining data
        self.send(SinkEvent::Shutdown);

        eprintln!("HttpEventSink::on_shutdown() - waiting for background thread to complete flush");
        // Wait for the background thread to signal that shutdown is complete
        let (lock, cvar) = &*self.shutdown_complete;
        let completed = lock.lock().unwrap();
        let timeout = std::time::Duration::from_secs(5);
        let result = cvar.wait_timeout_while(completed, timeout, |&mut c| !c);
        eprintln!("HttpEventSink::on_shutdown() - wait result: {:?}", result);
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
