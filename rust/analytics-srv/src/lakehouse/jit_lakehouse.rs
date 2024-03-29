use super::span_table::TabularSpanTree;
use crate::scope::ScopeHashMap;
use anyhow::Result;
use async_trait::async_trait;
use micromegas_analytics::process::ProcessEntry;

#[async_trait]
pub trait JitLakehouse: Send + Sync {
    // build_timeline_tables is for prototyping the use of deltalake
    #[cfg(feature = "deltalake-proto")]
    async fn build_timeline_tables(&self, process_id: &str) -> Result<()>;

    async fn get_thread_block(
        &self,
        process: &ProcessEntry,
        stream: &micromegas_telemetry_sink::stream_info::StreamInfo,
        block_id: &str,
    ) -> Result<BlockSpansReply>;

    async fn get_call_tree(
        &self,
        process: &ProcessEntry,
        stream: &micromegas_telemetry_sink::stream_info::StreamInfo,
        block_id: &str,
    ) -> Result<(ScopeHashMap, TabularSpanTree)>;
}
