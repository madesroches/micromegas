use analytics::prelude::*;
use anyhow::Context;
use anyhow::Result;
use lgn_blob_storage::BlobStorage;
use lgn_telemetry_proto::analytics::performance_analytics_server::PerformanceAnalytics;
use lgn_telemetry_proto::analytics::BlockSpansReply;
use lgn_telemetry_proto::analytics::BuildTimelineTablesReply;
use lgn_telemetry_proto::analytics::BuildTimelineTablesRequest;
use lgn_telemetry_proto::analytics::CumulativeCallGraphComputedBlock;
use lgn_telemetry_proto::analytics::FindProcessReply;
use lgn_telemetry_proto::analytics::FindProcessRequest;
use lgn_telemetry_proto::analytics::Level;
use lgn_telemetry_proto::analytics::ListProcessChildrenRequest;
use lgn_telemetry_proto::analytics::ListProcessStreamsRequest;
use lgn_telemetry_proto::analytics::ListStreamBlocksReply;
use lgn_telemetry_proto::analytics::ListStreamBlocksRequest;
use lgn_telemetry_proto::analytics::ListStreamsReply;
use lgn_telemetry_proto::analytics::MetricBlockData;
use lgn_telemetry_proto::analytics::MetricBlockManifest;
use lgn_telemetry_proto::analytics::MetricBlockManifestRequest;
use lgn_telemetry_proto::analytics::MetricBlockRequest;
use lgn_telemetry_proto::analytics::ProcessChildrenReply;
use lgn_telemetry_proto::analytics::ProcessListReply;
use lgn_telemetry_proto::analytics::ProcessLogReply;
use lgn_telemetry_proto::analytics::ProcessLogRequest;
use lgn_telemetry_proto::analytics::ProcessNbLogEntriesReply;
use lgn_telemetry_proto::analytics::ProcessNbLogEntriesRequest;
use lgn_telemetry_proto::analytics::RecentProcessesRequest;
use lgn_telemetry_proto::analytics::SearchProcessRequest;
use lgn_telemetry_proto::analytics::{
    BlockSpansRequest, ListProcessBlocksRequest, ProcessBlocksReply,
};
use lgn_telemetry_proto::analytics::{
    CumulativeCallGraphBlockRequest, CumulativeCallGraphManifest,
    CumulativeCallGraphManifestRequest,
};
use sqlx::PgPool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::dispatch::init_thread_stream;
use tracing::flush_monitor::FlushMonitor;
use tracing::prelude::*;

use crate::cache::DiskCache;
use crate::call_tree::reduce_lod;
use crate::cumulative_call_graph_handler::CumulativeCallGraphHandler;
use crate::lakehouse::jit_lakehouse::JitLakehouse;
use crate::log_entry::Searchable;
use crate::metrics::MetricHandler;

static REQUEST_COUNT: AtomicU64 = AtomicU64::new(0);

struct RequestGuard {
    begin_ticks: i64,
}

impl RequestGuard {
    fn new() -> Self {
        init_thread_stream();
        let previous_count = REQUEST_COUNT.fetch_add(1, Ordering::SeqCst);
        imetric!("Request Count", "count", previous_count);

        let begin_ticks = tracing::now();
        Self { begin_ticks }
    }
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        let end_ticks = tracing::now();
        let duration = end_ticks - self.begin_ticks;
        imetric!("Request Duration", "ticks", duration as u64);
    }
}

pub struct AnalyticsService {
    pool: PgPool,
    data_lake_blobs: Arc<dyn BlobStorage>,
    cache: Arc<DiskCache>,
    jit_lakehouse: Arc<dyn JitLakehouse>,
    flush_monitor: FlushMonitor,
}

impl AnalyticsService {
    #[span_fn]
    pub fn new(
        pool: PgPool,
        data_lake_blobs: Arc<dyn BlobStorage>,
        cache_blobs: Arc<dyn BlobStorage>,
        jit_lakehouse: Arc<dyn JitLakehouse>,
    ) -> Self {
        Self {
            pool,
            data_lake_blobs,
            cache: Arc::new(DiskCache::new(cache_blobs)),
            jit_lakehouse,
            flush_monitor: FlushMonitor::default(),
        }
    }

    #[span_fn]
    fn get_metric_handler(&self) -> MetricHandler {
        MetricHandler::new(
            Arc::clone(&self.data_lake_blobs),
            Arc::clone(&self.cache),
            self.pool.clone(),
        )
    }

    #[span_fn]
    async fn find_process_impl(&self, process_id: &str) -> Result<telemetry_sink::ProcessInfo> {
        let mut connection = self.pool.acquire().await?;
        find_process(&mut connection, process_id).await
    }

    #[span_fn]
    async fn list_recent_processes_impl(
        &self,
        parent_process_id: &str,
    ) -> Result<Vec<lgn_telemetry_proto::analytics::ProcessInstance>> {
        let mut connection = self.pool.acquire().await?;
        list_recent_processes(&mut connection, Some(parent_process_id)).await
    }

    #[span_fn]
    async fn search_processes_impl(
        &self,
        search: &str,
    ) -> Result<Vec<lgn_telemetry_proto::analytics::ProcessInstance>> {
        let mut connection = self.pool.acquire().await?;
        search_processes(&mut connection, search).await
    }

    #[span_fn]
    async fn list_process_streams_impl(
        &self,
        process_id: &str,
    ) -> Result<Vec<telemetry_sink::stream_info::StreamInfo>> {
        let mut connection = self.pool.acquire().await?;
        find_process_streams(&mut connection, process_id).await
    }

    #[span_fn]
    async fn list_stream_blocks_impl(
        &self,
        stream_id: &str,
    ) -> Result<Vec<lgn_telemetry_proto::telemetry::BlockMetadata>> {
        let mut connection = self.pool.acquire().await?;
        find_stream_blocks(&mut connection, stream_id).await
    }

    #[span_fn]
    async fn compute_spans_lod(
        &self,
        process: &telemetry_sink::ProcessInfo,
        stream: &telemetry_sink::stream_info::StreamInfo,
        block_id: &str,
        lod_id: u32,
    ) -> Result<BlockSpansReply> {
        let lod0_reply = self
            .jit_lakehouse
            .get_thread_block(process, stream, block_id)
            .await?;
        if lod_id == 0 {
            return Ok(lod0_reply);
        }
        let lod0 = lod0_reply.lod.unwrap();
        let reduced = reduce_lod(&lod0, lod_id);
        Ok(BlockSpansReply {
            scopes: lod0_reply.scopes,
            lod: Some(reduced),
            block_id: block_id.to_owned(),
            begin_ms: lod0_reply.begin_ms,
            end_ms: lod0_reply.end_ms,
        })
    }

    #[span_fn]
    async fn block_spans_impl(
        &self,
        process: &telemetry_sink::ProcessInfo,
        stream: &telemetry_sink::stream_info::StreamInfo,
        block_id: &str,
        lod_id: u32,
    ) -> Result<BlockSpansReply> {
        async_span_scope!("AnalyticsService::block_spans_impl");
        if lod_id == 0 {
            self.jit_lakehouse
                .get_thread_block(process, stream, block_id)
                .await
        } else {
            let cache_item_name = format!("spans_{}_{}", block_id, lod_id);
            self.cache
                .get_or_put(&cache_item_name, async {
                    self.compute_spans_lod(process, stream, block_id, lod_id)
                        .await
                })
                .await
        }
    }

    #[allow(clippy::cast_precision_loss)]
    #[span_fn]
    async fn process_log_impl(
        &self,
        process: &telemetry_sink::ProcessInfo,
        begin: u64,
        end: u64,
        search: &Option<String>,
        level_threshold: Option<Level>,
    ) -> Result<ProcessLogReply> {
        let mut connection = self.pool.acquire().await?;
        let mut entries = vec![];
        let mut entry_index: u64 = 0;

        let needles = match search {
            Some(search) if !search.is_empty() => Some(
                search
                    .split(' ')
                    .filter_map(|part| {
                        if part.is_empty() {
                            None
                        } else {
                            Some(part.to_lowercase())
                        }
                    })
                    .collect::<Vec<String>>(),
            ),
            _ => None,
        };

        for stream in find_process_log_streams(&mut connection, &process.process_id)
            .await
            .with_context(|| "error in find_process_log_streams")?
        {
            for block in find_stream_blocks(&mut connection, &stream.stream_id)
                .await
                .with_context(|| "error in find_stream_blocks")?
            {
                if (entry_index + block.nb_objects as u64) < begin {
                    entry_index += block.nb_objects as u64;
                } else {
                    for_each_log_entry_in_block(
                        self.data_lake_blobs.clone(),
                        process,
                        &stream,
                        &block,
                        |log_entry| {
                            if entry_index >= end {
                                return false;
                            }

                            if entry_index >= begin {
                                let valid_content = needles
                                    .as_ref()
                                    .map_or(true, |needles| log_entry.matches(needles.as_ref()));

                                let valid_level = level_threshold.map_or(true, |level_threshold| {
                                    log_entry.matches(level_threshold)
                                });

                                if valid_content && valid_level {
                                    entries.push(log_entry);
                                    entry_index += 1;
                                }
                            } else {
                                entry_index += 1;
                            }

                            true
                        },
                    )
                    .await
                    .with_context(|| "error in for_each_log_entry_in_block")?;
                }
            }
        }

        Ok(ProcessLogReply {
            entries,
            begin,
            end: entry_index,
        })
    }

    #[span_fn]
    async fn nb_process_log_entries_impl(
        &self,
        process_id: &str,
    ) -> Result<ProcessNbLogEntriesReply> {
        let mut connection = self.pool.acquire().await?;
        let mut count: u64 = 0;
        for stream in find_process_log_streams(&mut connection, process_id).await? {
            for b in find_stream_blocks(&mut connection, &stream.stream_id).await? {
                count += b.nb_objects as u64;
            }
        }
        Ok(ProcessNbLogEntriesReply { count })
    }

    #[span_fn]
    async fn list_process_children_impl(&self, process_id: &str) -> Result<ProcessChildrenReply> {
        let mut connection = self.pool.acquire().await?;
        let children = fetch_child_processes(&mut connection, process_id).await?;
        Ok(ProcessChildrenReply {
            processes: children,
        })
    }

    #[span_fn]
    async fn fetch_block_metric_impl(
        &self,
        request: MetricBlockRequest,
    ) -> Result<MetricBlockData> {
        let metric_handler = self.get_metric_handler();
        metric_handler.get_block_lod_data(request).await
    }

    #[span_fn]
    async fn fetch_block_metric_manifest_impl(
        &self,
        request: MetricBlockManifestRequest,
    ) -> Result<MetricBlockManifest> {
        let metric_handler = self.get_metric_handler();
        metric_handler
            .get_block_manifest(&request.process_id, &request.block_id, &request.stream_id)
            .await
    }

    #[span_fn]
    async fn list_process_blocks_impl(
        &self,
        request: ListProcessBlocksRequest,
    ) -> Result<ProcessBlocksReply> {
        let mut connection = self.pool.acquire().await?;
        let blocks =
            find_process_blocks(&mut connection, &request.process_id, &request.tag).await?;
        Ok(ProcessBlocksReply { blocks })
    }

    #[span_fn]
    #[allow(unused_variables)]
    async fn build_timeline_tables_impl(
        &self,
        request: BuildTimelineTablesRequest,
    ) -> Result<BuildTimelineTablesReply> {
        #[cfg(feature = "deltalake-proto")]
        self.jit_lakehouse
            .build_timeline_tables(&request.process_id)
            .await?;
        Ok(BuildTimelineTablesReply {})
    }
}

#[tonic::async_trait]
impl PerformanceAnalytics for AnalyticsService {
    async fn find_process(
        &self,
        request: Request<FindProcessRequest>,
    ) -> Result<Response<FindProcessReply>, Status> {
        self.flush_monitor.tick();
        async_span_scope!("AnalyticsService::find_process");
        let _guard = RequestGuard::new();
        info!("find_process");
        let find_request = request.into_inner();
        match self.find_process_impl(&find_request.process_id).await {
            Ok(process) => {
                let reply = FindProcessReply {
                    process: Some(process),
                };
                Ok(Response::new(reply))
            }
            Err(e) => {
                error!("Error in find_process: {:?}", e);
                return Err(Status::internal(format!("Error in find_process: {}", e)));
            }
        }
    }

    async fn list_recent_processes(
        &self,
        request: Request<RecentProcessesRequest>,
    ) -> Result<Response<ProcessListReply>, Status> {
        self.flush_monitor.tick();
        async_span_scope!("AnalyticsService::list_recent_processes");
        let _guard = RequestGuard::new();
        info!("list_recent_processes");
        let list_request = request.into_inner();
        match self
            .list_recent_processes_impl(&list_request.parent_process_id)
            .await
        {
            Ok(processes) => {
                let reply = ProcessListReply { processes };
                info!("list_recent_processes_impl ok");
                Ok(Response::new(reply))
            }
            Err(e) => {
                error!("Error in list_recent_processes_impl: {:?}", e);
                return Err(Status::internal(format!(
                    "Error in list_recent_processes_impl: {}",
                    e
                )));
            }
        }
    }

    async fn search_processes(
        &self,
        request: Request<SearchProcessRequest>,
    ) -> Result<Response<ProcessListReply>, Status> {
        self.flush_monitor.tick();
        async_span_scope!("AnalyticsService::search_processes");
        let _guard = RequestGuard::new();
        info!("search_processes");
        let inner = request.into_inner();
        debug!("{}", &inner.search);
        match self.search_processes_impl(&inner.search).await {
            Ok(processes) => {
                let reply = ProcessListReply { processes };
                info!("search_processes_impl ok");
                Ok(Response::new(reply))
            }
            Err(e) => {
                error!("Error in search_processes_impl: {:?}", e);
                return Err(Status::internal(format!(
                    "Error in search_processes_impl: {}",
                    e
                )));
            }
        }
    }

    async fn list_process_streams(
        &self,
        request: Request<ListProcessStreamsRequest>,
    ) -> Result<Response<ListStreamsReply>, Status> {
        self.flush_monitor.tick();
        async_span_scope!("AnalyticsService::list_process_streams");
        let _guard = RequestGuard::new();
        info!("list_process_streams");
        let list_request = request.into_inner();
        match self
            .list_process_streams_impl(&list_request.process_id)
            .await
        {
            Ok(streams) => {
                let reply = ListStreamsReply { streams };
                info!("list_process_streams ok");
                Ok(Response::new(reply))
            }
            Err(e) => {
                error!("Error in list_process_streams: {:?}", e);
                return Err(Status::internal(format!(
                    "Error in list_process_streams: {}",
                    e
                )));
            }
        }
    }

    async fn list_stream_blocks(
        &self,
        request: Request<ListStreamBlocksRequest>,
    ) -> Result<Response<ListStreamBlocksReply>, Status> {
        self.flush_monitor.tick();
        async_span_scope!("AnalyticsService::list_stream_blocks");
        let _guard = RequestGuard::new();
        let list_request = request.into_inner();
        match self.list_stream_blocks_impl(&list_request.stream_id).await {
            Ok(blocks) => {
                let reply = ListStreamBlocksReply { blocks };
                Ok(Response::new(reply))
            }
            Err(e) => {
                error!("Error in list_stream_blocks: {:?}", e);
                return Err(Status::internal(format!(
                    "Error in list_stream_blocks: {}",
                    e
                )));
            }
        }
    }

    async fn block_spans(
        &self,
        request: Request<BlockSpansRequest>,
    ) -> Result<Response<BlockSpansReply>, Status> {
        self.flush_monitor.tick();
        async_span_scope!("AnalyticsService::block_spans");
        let _guard = RequestGuard::new();
        let inner_request = request.into_inner();
        if inner_request.process.is_none() {
            error!("Missing process in block_spans");
            return Err(Status::internal(String::from(
                "Missing process in block_spans",
            )));
        }
        if inner_request.stream.is_none() {
            error!("Missing stream in block_spans");
            return Err(Status::internal(String::from(
                "Missing stream in block_spans",
            )));
        }

        match self
            .block_spans_impl(
                &inner_request.process.unwrap(),
                &inner_request.stream.unwrap(),
                &inner_request.block_id,
                inner_request.lod_id,
            )
            .await
        {
            Ok(block_spans) => Ok(Response::new(block_spans)),
            Err(e) => {
                error!("Error in block_spans: {:?}", e);
                return Err(Status::internal(format!("Error in block_spans: {}", e)));
            }
        }
    }

    async fn fetch_cumulative_call_graph_manifest(
        &self,
        request: Request<CumulativeCallGraphManifestRequest>,
    ) -> Result<Response<CumulativeCallGraphManifest>, Status> {
        self.flush_monitor.tick();
        async_span_scope!("AnalyticsService::fetch_cumulative_call_graph_manifest");
        let _guard = RequestGuard::new();
        let inner_request = request.into_inner();
        let handler =
            CumulativeCallGraphHandler::new(self.pool.clone(), self.jit_lakehouse.clone());
        match handler
            .get_process_call_graph_manifest(
                inner_request.process_id,
                inner_request.begin_ms,
                inner_request.end_ms,
            )
            .await
        {
            Ok(reply) => Ok(Response::new(reply)),
            Err(e) => {
                error!("Error in fetch_cumulative_call_graph_manifest: {:?}", e);
                Err(Status::internal(format!(
                    "Error in fetch_cumulative_call_graph_manifest: {}",
                    e
                )))
            }
        }
    }

    async fn fetch_cumulative_call_graph_computed_block(
        &self,
        request: tonic::Request<CumulativeCallGraphBlockRequest>,
    ) -> Result<tonic::Response<CumulativeCallGraphComputedBlock>, Status> {
        self.flush_monitor.tick();
        async_span_scope!("AnalyticsService::fetch_cumulative_call_graph_computed_block");
        let _guard = RequestGuard::new();
        let inner_request = request.into_inner();
        let handler =
            CumulativeCallGraphHandler::new(self.pool.clone(), self.jit_lakehouse.clone());
        match handler
            .get_call_graph_computed_block(
                inner_request.block_id,
                inner_request.start_ticks,
                inner_request.tsc_frequency,
                inner_request.begin_ms,
                inner_request.end_ms,
            )
            .await
        {
            Ok(reply) => Ok(Response::new(reply)),
            Err(e) => {
                error!(
                    "Error in fetch_cumulative_call_graph_computed_block: {:?}",
                    e
                );
                Err(Status::internal(format!(
                    "Error in fetch_cumulative_call_graph_computed_block: {}",
                    e
                )))
            }
        }
    }

    async fn list_process_log_entries(
        &self,
        request: Request<ProcessLogRequest>,
    ) -> Result<Response<ProcessLogReply>, Status> {
        self.flush_monitor.tick();
        async_span_scope!("AnalyticsService::list_process_log_entries");
        let _guard = RequestGuard::new();
        let inner_request = request.into_inner();
        let process = match inner_request.process {
            Some(process) => process,
            None => {
                error!("Missing process in list_process_log_entries");
                return Err(Status::internal(String::from(
                    "Missing process in list_process_log_entries",
                )));
            }
        };

        match self
            .process_log_impl(
                &process,
                inner_request.begin,
                inner_request.end,
                &inner_request.search,
                inner_request.level_threshold.and_then(Level::from_i32),
            )
            .await
        {
            Ok(reply) => Ok(Response::new(reply)),
            Err(e) => {
                error!("Error in list_process_log_entries: {:?}", e);
                Err(Status::internal(format!(
                    "Error in list_process_log_entries: {}",
                    e
                )))
            }
        }
    }

    async fn nb_process_log_entries(
        &self,
        request: Request<ProcessNbLogEntriesRequest>,
    ) -> Result<Response<ProcessNbLogEntriesReply>, Status> {
        self.flush_monitor.tick();
        async_span_scope!("AnalyticsService::nb_process_log_entries");
        let _guard = RequestGuard::new();
        let inner_request = request.into_inner();
        if inner_request.process_id.is_empty() {
            error!("Missing process_id in nb_process_log_entries");
            return Err(Status::internal(String::from(
                "Missing process_id in nb_process_log_entries",
            )));
        }
        match self
            .nb_process_log_entries_impl(&inner_request.process_id)
            .await
        {
            Ok(reply) => Ok(Response::new(reply)),
            Err(e) => {
                error!("Error in nb_process_log_entries: {:?}", e);
                Err(Status::internal(format!(
                    "Error in nb_process_log_entries: {}",
                    e
                )))
            }
        }
    }

    async fn list_process_children(
        &self,
        request: Request<ListProcessChildrenRequest>,
    ) -> Result<Response<ProcessChildrenReply>, Status> {
        self.flush_monitor.tick();
        async_span_scope!("AnalyticsService::list_process_children");
        let _guard = RequestGuard::new();
        let inner_request = request.into_inner();
        if inner_request.process_id.is_empty() {
            error!("Missing process_id in list_process_children");
            return Err(Status::internal(String::from(
                "Missing process_id in list_process_children",
            )));
        }
        match self
            .list_process_children_impl(&inner_request.process_id)
            .await
        {
            Ok(reply) => Ok(Response::new(reply)),
            Err(e) => {
                error!("Error in list_process_children: {:?}", e);
                Err(Status::internal(format!(
                    "Error in list_process_children: {}",
                    e
                )))
            }
        }
    }

    async fn list_process_blocks(
        &self,
        request: Request<ListProcessBlocksRequest>,
    ) -> Result<Response<ProcessBlocksReply>, Status> {
        self.flush_monitor.tick();
        async_span_scope!("AnalyticsService::list_process_blocks");
        let _guard = RequestGuard::new();
        let inner_request = request.into_inner();
        match self.list_process_blocks_impl(inner_request).await {
            Ok(reply) => Ok(Response::new(reply)),
            Err(e) => {
                error!("Error in list_process_blocks: {:?}", e);
                Err(Status::internal(format!(
                    "Error in list_process_blocks: {}",
                    e
                )))
            }
        }
    }

    async fn fetch_block_metric(
        &self,
        request: Request<MetricBlockRequest>,
    ) -> Result<Response<MetricBlockData>, Status> {
        self.flush_monitor.tick();
        async_span_scope!("AnalyticsService::fetch_block_metric");
        let _guard = RequestGuard::new();
        let inner_request = request.into_inner();
        match self.fetch_block_metric_impl(inner_request).await {
            Ok(reply) => Ok(Response::new(reply)),
            Err(e) => {
                error!("Error in fetch_block_metric: {:?}", e);
                Err(Status::internal(format!(
                    "Error in fetch_block_metric: {}",
                    e
                )))
            }
        }
    }

    async fn fetch_block_metric_manifest(
        &self,
        request: Request<MetricBlockManifestRequest>,
    ) -> Result<Response<MetricBlockManifest>, Status> {
        self.flush_monitor.tick();
        async_span_scope!("AnalyticsService::fetch_block_metric_manifest");
        let _guard = RequestGuard::new();
        let inner_request = request.into_inner();
        match self.fetch_block_metric_manifest_impl(inner_request).await {
            Ok(reply) => Ok(Response::new(reply)),
            Err(e) => {
                error!("Error in fetch_block_metric_manifest: {:?}", e);
                Err(Status::internal(format!(
                    "Error in fetch_block_metric_manifest: {}",
                    e
                )))
            }
        }
    }

    #[span_fn]
    async fn build_timeline_tables(
        &self,
        request: Request<BuildTimelineTablesRequest>,
    ) -> Result<Response<BuildTimelineTablesReply>, Status> {
        self.flush_monitor.tick();
        async_span_scope!("AnalyticsService::build_timeline_tables");
        let _guard = RequestGuard::new();
        let inner_request = request.into_inner();
        match self.build_timeline_tables_impl(inner_request).await {
            Ok(reply) => Ok(Response::new(reply)),
            Err(e) => {
                error!("Error in build_timeline_tables: {:?}", e);
                Err(Status::internal(format!(
                    "Error in build_timeline_tables: {}",
                    e
                )))
            }
        }
    }
}
