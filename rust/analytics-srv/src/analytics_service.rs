use anyhow::Context;
use anyhow::Result;
use lgn_blob_storage::BlobStorage;
use micromegas_analytics::block::BlockMetadata;
use micromegas_analytics::prelude::*;
use micromegas_analytics::process::ProcessEntry;
use micromegas_tracing::dispatch::init_thread_stream;
use micromegas_tracing::flush_monitor::FlushMonitor;
use micromegas_tracing::prelude::*;
use sqlx::PgPool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tonic::{Request, Response, Status};

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

        let begin_ticks = micromegas_tracing::now();
        Self { begin_ticks }
    }
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        let end_ticks = micromegas_tracing::now();
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
    async fn find_process_impl(&self, process_id: &str) -> Result<ProcessEntry> {
        let mut connection = self.pool.acquire().await?;
        find_process(&mut connection, process_id).await
    }

    #[span_fn]
    async fn list_recent_processes_impl(
        &self,
        parent_process_id: &str,
    ) -> Result<Vec<ProcessEntry>> {
        let mut connection = self.pool.acquire().await?;
        list_recent_processes(&mut connection, Some(parent_process_id)).await
    }

    #[span_fn]
    async fn search_processes_impl(&self, search: &str) -> Result<Vec<ProcessEntry>> {
        let mut connection = self.pool.acquire().await?;
        search_processes(&mut connection, search).await
    }

    #[span_fn]
    async fn list_process_streams_impl(
        &self,
        process_id: &str,
    ) -> Result<Vec<micromegas_telemetry_sink::stream_info::StreamInfo>> {
        let mut connection = self.pool.acquire().await?;
        find_process_streams(&mut connection, process_id).await
    }

    #[span_fn]
    async fn list_stream_blocks_impl(&self, stream_id: &str) -> Result<Vec<BlockMetadata>> {
        let mut connection = self.pool.acquire().await?;
        find_stream_blocks(&mut connection, stream_id).await
    }

    #[span_fn]
    async fn compute_spans_lod(
        &self,
        process: &ProcessEntry,
        stream: &micromegas_telemetry_sink::stream_info::StreamInfo,
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
        process: &micromegas_telemetry_sink::ProcessInfo,
        stream: &micromegas_telemetry_sink::stream_info::StreamInfo,
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
        process: &micromegas_telemetry_sink::ProcessInfo,
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
