use std::collections::HashMap;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::{
    fmt,
    sync::{Arc, Mutex},
};
use tracing::ProcessInfo;

use async_trait::async_trait;
use lgn_auth::{Authenticator, ClientTokenSet};

use tracing::{
    event::EventSink,
    logs::{LogBlock, LogMetadata, LogStream},
    metrics::{MetricsBlock, MetricsStream},
    prelude::*,
    spans::{ThreadBlock, ThreadStream},
};

use crate::stream_block::StreamBlock;
use crate::stream_info::{get_stream_info, StreamInfo};

#[derive(Debug)]
enum SinkEvent {
    Startup(Arc<ProcessInfo>),
    InitStream(Arc<StreamInfo>),
    ProcessLogBlock(Arc<LogBlock>),
    ProcessMetricsBlock(Arc<MetricsBlock>),
    ProcessThreadBlock(Arc<ThreadBlock>),
}

#[derive(Clone, Debug)]
struct StaticApiKey {}

#[async_trait]
impl Authenticator for StaticApiKey {
    async fn login(
        &self,
        _scopes: &[String],
        _extra_params: &Option<HashMap<String, String>>,
    ) -> lgn_auth::Result<ClientTokenSet> {
        Ok(ClientTokenSet {
            access_token: env!("LGN_TELEMETRY_GRPC_API_KEY").to_owned(), //todo: remove this variable
            refresh_token: None,
            id_token: None,
            token_type: String::from("Legion API Key"),
            expires_in: None,
            scopes: None,
        })
    }
    async fn refresh_login(
        &self,
        _client_token_set: ClientTokenSet,
    ) -> lgn_auth::Result<ClientTokenSet> {
        self.login(&[], &None).await
    }
    async fn logout(&self) -> lgn_auth::Result<()> {
        Ok(())
    }
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
    pub fn new(addr_server: &str, max_queue_size: isize) -> Self {
        let addr = addr_server.to_owned();
        let (sender, receiver) = std::sync::mpsc::channel::<SinkEvent>();
        let queue_size = Arc::new(AtomicIsize::new(0));
        let thread_queue_size = queue_size.clone();
        Self {
            thread: Some(std::thread::spawn(move || {
                Self::thread_proc(addr, receiver, thread_queue_size, max_queue_size);
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
    ) {
        info!("sending process {process_info:?}");
        let url = format!("{root_path}/ingestion/insert_process");
        info!("http post: {url}");
        let request = client.post(url).json(&*process_info);
        info!("request: {request:?}");
        match request.send().await {
            Ok(response) => {
                info!("http response: {response:?}");
            }
            Err(e) => {
                eprintln!("insert_process failed: {e:?}");
            }
        }
    }

    async fn push_stream(
        client: &mut reqwest::Client,
        root_path: &str,
        stream_info: Arc<StreamInfo>,
    ) {
        let body_res = serde_json::to_string(&*stream_info);
        if let Err(e) = body_res {
            eprintln!("error serializing stream_desc: {e:?}");
            return;
        }
        match client
            .post(format!("{root_path}/ingestion/insert_stream"))
            .body(body_res.unwrap())
            .send()
            .await
        {
            Ok(_response) => {}
            Err(e) => {
                eprintln!("insert_stream failed: {e:?}");
            }
        }
    }

    async fn push_block(
        client: &mut reqwest::Client,
        root_path: &str,
        buffer: &dyn StreamBlock,
        current_queue_size: &AtomicIsize,
        max_queue_size: isize,
    ) {
        if current_queue_size.load(Ordering::Relaxed) >= max_queue_size {
            // could be better to have a budget for each block type
            // this way thread data would not starve the other streams
            return;
        }
        match buffer.encode_bin() {
            Ok(encoded_block) => match client
                .post(format!("{root_path}/ingestion/insert_block"))
                .body(encoded_block)
                .send()
                .await
            {
                Ok(_response) => {}
                Err(e) => {
                    eprintln!("insert_block failed: {}", e);
                }
            },
            Err(e) => {
                eprintln!("block encoding failed: {}", e);
            }
        }
    }

    async fn thread_proc_impl(
        addr: String,
        receiver: std::sync::mpsc::Receiver<SinkEvent>,
        queue_size: Arc<AtomicIsize>,
        max_queue_size: isize,
    ) {
        let client_res = reqwest::Client::builder().build();
        if let Err(e) = client_res {
            eprintln!("Error creating http client: {e:?}");
            return;
        }
        let mut client = client_res.unwrap();
        // eagerly connect, a new process message is sure to follow if it's not already in queue
        if let Some(process_id) = tracing::dispatch::process_id() {
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
        loop {
            match receiver.recv() {
                Ok(message) => match message {
                    SinkEvent::Startup(process_info) => {
                        Self::push_process(&mut client, &addr, process_info).await;
                    }
                    SinkEvent::InitStream(stream_info) => {
                        Self::push_stream(&mut client, &addr, stream_info).await;
                    }
                    SinkEvent::ProcessLogBlock(buffer) => {
                        Self::push_block(&mut client, &addr, &*buffer, &queue_size, max_queue_size)
                            .await;
                    }
                    SinkEvent::ProcessMetricsBlock(buffer) => {
                        Self::push_block(&mut client, &addr, &*buffer, &queue_size, max_queue_size)
                            .await;
                    }
                    SinkEvent::ProcessThreadBlock(buffer) => {
                        Self::push_block(&mut client, &addr, &*buffer, &queue_size, max_queue_size)
                            .await;
                    }
                },
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
    ) {
        // TODO: add runtime as configuration option (or create one only if global don't exist)
        let tokio_runtime = tokio::runtime::Runtime::new().unwrap();
        tokio_runtime.block_on(Self::thread_proc_impl(
            addr,
            receiver,
            queue_size,
            max_queue_size,
        ));
    }
}

impl EventSink for HttpEventSink {
    fn on_startup(&self, process_info: Arc<tracing::ProcessInfo>) {
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
        self.send(SinkEvent::InitStream(Arc::new(get_stream_info(log_stream))));
    }

    fn on_process_log_block(&self, log_block: Arc<LogBlock>) {
        self.send(SinkEvent::ProcessLogBlock(log_block));
    }

    fn on_init_metrics_stream(&self, metrics_stream: &MetricsStream) {
        self.send(SinkEvent::InitStream(Arc::new(get_stream_info(
            metrics_stream,
        ))));
    }

    fn on_process_metrics_block(&self, metrics_block: Arc<MetricsBlock>) {
        self.send(SinkEvent::ProcessMetricsBlock(metrics_block));
    }

    fn on_init_thread_stream(&self, thread_stream: &ThreadStream) {
        self.send(SinkEvent::InitStream(Arc::new(get_stream_info(
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
