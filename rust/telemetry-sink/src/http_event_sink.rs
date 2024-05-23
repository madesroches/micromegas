use anyhow::{Context, Result};
use micromegas_telemetry::stream_info::StreamInfo;
use micromegas_telemetry::wire_format::encode_cbor;
use micromegas_tracing::{
    event::EventSink,
    flush_monitor::FlushMonitor,
    logs::{LogBlock, LogMetadata, LogStream},
    metrics::{MetricsBlock, MetricsStream},
    prelude::*,
    spans::{ThreadBlock, ThreadStream},
};
use std::{
    cmp::min,
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
}

pub struct HttpEventSink {
    thread: Option<std::thread::JoinHandle<()>>,
    // TODO: simplify this?
    sender: Mutex<Option<std::sync::mpsc::Sender<SinkEvent>>>,
    queue_size: Arc<AtomicIsize>,
}

impl Drop for HttpEventSink {
    fn drop(&mut self) {
        let mut sender_guard = self.sender.lock().unwrap();
        *sender_guard = None;
        if let Some(handle) = self.thread.take() {
            handle.join().expect("Error joining telemetry thread");
        }
    }
}

impl HttpEventSink {
    pub fn new(
        addr_server: &str,
        max_queue_size: isize,
        metadata_retry: core::iter::Take<tokio_retry::strategy::ExponentialBackoff>,
        make_decorator: Box<dyn FnOnce() -> Arc<dyn RequestDecorator> + Send>,
    ) -> Self {
        let addr = addr_server.to_owned();
        let (sender, receiver) = std::sync::mpsc::channel::<SinkEvent>();
        let queue_size = Arc::new(AtomicIsize::new(0));
        let thread_queue_size = queue_size.clone();
        Self {
            thread: Some(std::thread::spawn(move || {
                Self::thread_proc(
                    addr,
                    receiver,
                    thread_queue_size,
                    max_queue_size,
                    metadata_retry,
                    make_decorator,
                );
            })),
            sender: Mutex::new(Some(sender)),
            queue_size,
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

    async fn push_process(
        client: &mut reqwest::Client,
        root_path: &str,
        process_info: Arc<ProcessInfo>,
        retry_strategy: core::iter::Take<tokio_retry::strategy::ExponentialBackoff>,
        decorator: &dyn RequestDecorator,
    ) -> Result<()> {
        debug!("sending process {process_info:?}");
        let url = format!("{root_path}/ingestion/insert_process");
        tokio_retry::Retry::spawn(retry_strategy, || async {
            let body = encode_cbor(&*process_info)?;
            let mut request = client.post(&url).body(body).build()?;
            decorator
                .decorate(&mut request)
                .await
                .with_context(|| "decorating request")?;
            let result = client
                .execute(request)
                .await
                .with_context(|| "executing request");
            if let Err(e) = &result {
                debug!("insert_process error: {e:?}");
            }
            result
        })
        .await?;
        Ok(())
    }

    async fn push_stream(
        client: &mut reqwest::Client,
        root_path: &str,
        stream_info: Arc<StreamInfo>,
        retry_strategy: core::iter::Take<tokio_retry::strategy::ExponentialBackoff>,
        decorator: &dyn RequestDecorator,
    ) -> Result<()> {
        let url = format!("{root_path}/ingestion/insert_stream");
        tokio_retry::Retry::spawn(retry_strategy, || async {
            let body = encode_cbor(&*stream_info)?;
            let mut request = client.post(&url).body(body).build()?;
            decorator.decorate(&mut request).await?;
            let result = client
                .execute(request)
                .await
                .with_context(|| "executing request");
            if let Err(e) = &result {
                debug!("insert_stream error: {e}");
            }
            result
        })
        .await?;
        Ok(())
    }

    async fn push_block(
        client: &mut reqwest::Client,
        root_path: &str,
        buffer: &dyn StreamBlock,
        current_queue_size: &AtomicIsize,
        max_queue_size: isize,
        decorator: &dyn RequestDecorator,
        process_info: &ProcessInfo,
    ) -> Result<()> {
        if current_queue_size.load(Ordering::Relaxed) >= max_queue_size {
            // could be better to have a budget for each block type
            // this way thread data would not starve the other streams
            debug!("dropping data, queue over max_queue_size");
            return Ok(());
        }
        let encoded_block = buffer.encode_bin(process_info)?;
        let mut request = client
            .post(format!("{root_path}/ingestion/insert_block"))
            .body(encoded_block)
            .build()?;
        decorator.decorate(&mut request).await?;
        client.execute(request).await?;
        Ok(())
    }

    async fn thread_proc_impl(
        addr: String,
        receiver: std::sync::mpsc::Receiver<SinkEvent>,
        queue_size: Arc<AtomicIsize>,
        max_queue_size: isize,
        retry_strategy: core::iter::Take<tokio_retry::strategy::ExponentialBackoff>,
        decorator: &dyn RequestDecorator,
    ) {
        let mut opt_process_info = None;
        let client_res = reqwest::Client::builder().build();
        if let Err(e) = client_res {
            error!("Error creating http client: {e:?}");
            return;
        }
        let mut client = client_res.unwrap();
        // eagerly connect, a new process message is sure to follow if it's not already in queue
        if let Some(process_id) = micromegas_tracing::dispatch::process_id() {
            info!("log: https://analytics.legionengine.com/log/{}", process_id);
            info!(
                "metrics: https://analytics.legionengine.com/metrics/{}",
                process_id
            );
            info!(
                "timeline: https://analytics.legionengine.com/timeline/{}",
                process_id
            );
        }
        let flusher = FlushMonitor::default();
        loop {
            let timeout = min(0, flusher.time_to_flush_seconds());
            match receiver.recv_timeout(Duration::from_secs(timeout as u64)) {
                Ok(message) => match message {
                    SinkEvent::Startup(process_info) => {
                        opt_process_info = Some(process_info.clone());
                        if let Err(e) = Self::push_process(
                            &mut client,
                            &addr,
                            process_info,
                            retry_strategy.clone(),
                            decorator,
                        )
                        .await
                        {
                            error!("error sending process: {e:?}");
                        }
                    }
                    SinkEvent::InitStream(stream_info) => {
                        if let Err(e) = Self::push_stream(
                            &mut client,
                            &addr,
                            stream_info,
                            retry_strategy.clone(),
                            decorator,
                        )
                        .await
                        {
                            error!("error sending stream: {e:?}");
                        }
                    }
                    SinkEvent::ProcessLogBlock(buffer) => {
                        if let Some(process_info) = &opt_process_info {
                            if let Err(e) = Self::push_block(
                                &mut client,
                                &addr,
                                &*buffer,
                                &queue_size,
                                max_queue_size,
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
                        if let Some(process_info) = &opt_process_info {
                            if let Err(e) = Self::push_block(
                                &mut client,
                                &addr,
                                &*buffer,
                                &queue_size,
                                max_queue_size,
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
                        if let Some(process_info) = &opt_process_info {
                            if let Err(e) = Self::push_block(
                                &mut client,
                                &addr,
                                &*buffer,
                                &queue_size,
                                max_queue_size,
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

    #[allow(clippy::needless_pass_by_value)] // we don't want to leave the receiver in the calling thread
    fn thread_proc(
        addr: String,
        receiver: std::sync::mpsc::Receiver<SinkEvent>,
        queue_size: Arc<AtomicIsize>,
        max_queue_size: isize,
        retry_strategy: core::iter::Take<tokio_retry::strategy::ExponentialBackoff>,
        make_decorator: Box<dyn FnOnce() -> Arc<dyn RequestDecorator> + Send>,
    ) {
        // TODO: add runtime as configuration option (or create one only if global don't exist)
        let tokio_runtime = tokio::runtime::Runtime::new().unwrap();
        let decorator = make_decorator();
        tokio_runtime.block_on(Self::thread_proc_impl(
            addr,
            receiver,
            queue_size,
            max_queue_size,
            retry_strategy,
            decorator.as_ref(),
        ));
    }
}

impl EventSink for HttpEventSink {
    fn on_startup(&self, process_info: Arc<ProcessInfo>) {
        self.send(SinkEvent::Startup(process_info));
    }

    fn on_shutdown(&self) {
        // nothing to do
    }

    fn on_log_enabled(&self, _metadata: &LogMetadata) -> bool {
        // If all previous filter succeeds this sink always agrees
        true
    }

    fn on_log(&self, _metadata: &LogMetadata, _time: i64, _args: fmt::Arguments<'_>) {}

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
