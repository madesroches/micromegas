use std::collections::HashMap;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::{
    fmt,
    sync::{Arc, Mutex},
};
use tracing::debug;

use async_trait::async_trait;
use lgn_auth::{Authenticator, ClientTokenSet};
use tonic::codegen::http::Uri;

use lgn_telemetry_proto::{
    ingestion::telemetry_ingestion_client::TelemetryIngestionClient,
    telemetry::{Process as ProcessInfo, Stream as StreamProto},
};
use tracing::{
    event::EventSink,
    logs::{LogBlock, LogMetadata, LogStream},
    metrics::{MetricsBlock, MetricsStream},
    prelude::*,
    spans::{ThreadBlock, ThreadStream},
};

use crate::stream_block::StreamBlock;
use crate::stream_info::get_stream_info;

#[derive(Debug)]
enum SinkEvent {
    Startup(ProcessInfo),
    InitStream(StreamProto),
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
            access_token: env!("LGN_TELEMETRY_GRPC_API_KEY").to_owned(),
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

type AuthClientType = TelemetryIngestionClient<
    lgn_online::client::AuthenticatedClient<lgn_online::grpc::GrpcClient, StaticApiKey>,
>;

fn connect_grpc_client(uri: Uri) -> AuthClientType {
    let grpc_client = lgn_online::grpc::GrpcClient::new(uri);
    let auth_client = lgn_online::client::AuthenticatedClient::new(
        grpc_client,
        Some(StaticApiKey {}),
        &Vec::new(),
    );
    TelemetryIngestionClient::new(auth_client)
}

pub struct GRPCEventSink {
    thread: Option<std::thread::JoinHandle<()>>,
    // TODO: simplify this?
    sender: Mutex<Option<std::sync::mpsc::Sender<SinkEvent>>>,
    queue_size: Arc<AtomicIsize>,
}

impl Drop for GRPCEventSink {
    fn drop(&mut self) {
        let mut sender_guard = self.sender.lock().unwrap();
        *sender_guard = None;
        if let Some(handle) = self.thread.take() {
            handle.join().expect("Error joining telemetry thread");
        }
    }
}

impl GRPCEventSink {
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

    async fn push_block(
        client: &mut AuthClientType,
        buffer: &dyn StreamBlock,
        current_queue_size: &AtomicIsize,
        max_queue_size: isize,
    ) {
        if current_queue_size.load(Ordering::Relaxed) >= max_queue_size {
            // could be better to have a budget for each block type
            // this way thread data would not starve the other streams
            return;
        }
        match buffer.encode() {
            Ok(encoded_block) => match client.insert_block(encoded_block).await {
                Ok(_response) => {}
                Err(e) => {
                    println!("insert_block failed: {}", e);
                }
            },
            Err(e) => {
                println!("block encoding failed: {}", e);
            }
        }
    }

    async fn thread_proc_impl(
        addr: String,
        receiver: std::sync::mpsc::Receiver<SinkEvent>,
        queue_size: Arc<AtomicIsize>,
        max_queue_size: isize,
    ) {
        let parsed_uri = addr.parse::<Uri>();
        if let Err(e) = parsed_uri {
            println!("Error parsing telemetry uri {}: {}", addr, e);
            return;
        }
        let uri = parsed_uri.unwrap();
        // eagerly connect, a new process message is sure to follow if it's not already in queue
        let mut client_store = Some(connect_grpc_client(uri.clone()));
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
                Ok(message) => {
                    let mut client = match client_store {
                        Some(c) => c,
                        None => connect_grpc_client(uri.clone()),
                    };
                    client_store = None;

                    match message {
                        SinkEvent::Startup(process_info) => {
                            match client.insert_process(process_info).await {
                                Ok(_response) => {}
                                Err(e) => {
                                    debug!("insert_process failed: {}", e);
                                }
                            }
                        }
                        SinkEvent::InitStream(stream_info) => {
                            match client.insert_stream(stream_info).await {
                                Ok(_response) => {}
                                Err(e) => {
                                    debug!("insert_stream failed: {}", e);
                                }
                            }
                        }
                        SinkEvent::ProcessLogBlock(buffer) => {
                            Self::push_block(&mut client, &*buffer, &*queue_size, max_queue_size)
                                .await;
                        }
                        SinkEvent::ProcessMetricsBlock(buffer) => {
                            Self::push_block(&mut client, &*buffer, &*queue_size, max_queue_size)
                                .await;
                        }
                        SinkEvent::ProcessThreadBlock(buffer) => {
                            Self::push_block(&mut client, &*buffer, &*queue_size, max_queue_size)
                                .await;
                        }
                    }

                    if queue_size.load(Ordering::Relaxed) >= 2 {
                        // don't keep the connection alive if there is nothing to send anymore.
                        // idle connections can be dropped on the server-side
                        client_store = Some(client);
                    }
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

impl EventSink for GRPCEventSink {
    fn on_startup(&self, process_info: tracing::ProcessInfo) {
        self.send(SinkEvent::Startup(ProcessInfo {
            process_id: process_info.process_id,
            exe: process_info.exe,
            username: process_info.username,
            realname: process_info.realname,
            computer: process_info.computer,
            distro: process_info.distro,
            cpu_brand: process_info.cpu_brand,
            tsc_frequency: process_info.tsc_frequency,
            start_time: process_info.start_time,
            start_ticks: process_info.start_ticks,
            parent_process_id: process_info.parent_process_id,
        }));
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
        self.send(SinkEvent::InitStream(get_stream_info(log_stream)));
    }

    fn on_process_log_block(&self, log_block: Arc<LogBlock>) {
        self.send(SinkEvent::ProcessLogBlock(log_block));
    }

    fn on_init_metrics_stream(&self, metrics_stream: &MetricsStream) {
        self.send(SinkEvent::InitStream(get_stream_info(metrics_stream)));
    }

    fn on_process_metrics_block(&self, metrics_block: Arc<MetricsBlock>) {
        self.send(SinkEvent::ProcessMetricsBlock(metrics_block));
    }

    fn on_init_thread_stream(&self, thread_stream: &ThreadStream) {
        self.send(SinkEvent::InitStream(get_stream_info(thread_stream)));
    }

    fn on_process_thread_block(&self, thread_block: Arc<ThreadBlock>) {
        self.send(SinkEvent::ProcessThreadBlock(thread_block));
    }

    fn is_busy(&self) -> bool {
        self.queue_size.load(Ordering::Relaxed) > 0
    }
}
