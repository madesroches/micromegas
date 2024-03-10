use std::path::Path;
use std::{path::PathBuf, sync::Arc};

#[cfg(feature = "deltalake-proto")]
use super::span_delta_table::update_spans_delta_table;

use super::span_table::{
    build_span_tree, build_spans_lod0, lod0_from_span_tree, make_rows_from_tree,
    write_spans_parquet, SpanRowGroup, TabularSpanTree,
};
use crate::lakehouse::bytes_chunk_reader::BytesChunkReader;
use crate::scope::ScopeHashMap;
use crate::{call_tree::process_thread_block, lakehouse::jit_lakehouse::JitLakehouse};
use analytics::time::ConvertTicks;
use anyhow::{Context, Result};
use async_trait::async_trait;
use lgn_blob_storage::BlobStorage;
use lgn_telemetry_proto::analytics::{BlockSpansReply, ScopeDesc, SpanBlockLod};
use parquet::file::reader::FileReader;
use parquet::file::serialized_reader::SerializedFileReader;
use parquet::record::RowAccessor;
use tokio::io::AsyncReadExt;
use tracing::prelude::*;

use super::scope_table::write_scopes_parquet;

pub struct LocalJitLakehouse {
    pool: sqlx::any::AnyPool,
    blob_storage: Arc<dyn BlobStorage>,
    tables_path: PathBuf,
}

impl LocalJitLakehouse {
    pub fn new(
        pool: sqlx::any::AnyPool,
        blob_storage: Arc<dyn BlobStorage>,
        tables_path: PathBuf,
    ) -> Self {
        Self {
            pool,
            blob_storage,
            tables_path,
        }
    }

    async fn write_call_tree(
        &self,
        process: &telemetry_sink::ProcessInfo,
        stream: &telemetry_sink::stream_info::StreamInfo,
        block_id: &str,
        spans_file_path: PathBuf,
        scopes_file_path: PathBuf,
    ) -> Result<(ScopeHashMap, TabularSpanTree)> {
        info!("writing thread block {}", block_id);
        if let Some(parent) = spans_file_path.parent() {
            tokio::fs::create_dir_all(&parent)
                .await
                .with_context(|| format!("creating directory {}", parent.display()))?;
        }
        if let Some(parent) = scopes_file_path.parent() {
            tokio::fs::create_dir_all(&parent)
                .await
                .with_context(|| format!("creating directory {}", parent.display()))?;
        }

        let convert_ticks = ConvertTicks::new(process);
        let processed = process_thread_block(
            self.pool.clone(),
            self.blob_storage.clone(),
            convert_ticks,
            stream,
            block_id,
        )
        .await?;
        if processed.call_tree_root.is_none() {
            return Ok((ScopeHashMap::new(), TabularSpanTree::new()));
        }
        let root = processed
            .call_tree_root
            .with_context(|| "reading root of call tree")?;
        let mut next_id = 1;
        let mut rows = SpanRowGroup::new();
        make_rows_from_tree(&root, &mut next_id, &mut rows);
        write_spans_parquet(&rows, &spans_file_path).await?;
        write_scopes_parquet(&processed.scopes, &scopes_file_path).await?;

        Ok((processed.scopes, TabularSpanTree::from_rows(&rows)?))
    }

    async fn read_spans_lod0(&self, spans_file_path: &Path) -> Result<SpanBlockLod> {
        let mut buffer: Vec<u8> = Vec::new();
        let mut file = tokio::fs::File::open(spans_file_path).await?;
        file.read_to_end(&mut buffer).await?;
        let bytes = BytesChunkReader {
            bytes: bytes::Bytes::from(buffer),
        };
        let file_reader = SerializedFileReader::new(bytes)?;
        build_spans_lod0(&file_reader)
    }

    async fn read_scopes(&self, spans_file_path: &Path) -> Result<ScopeHashMap> {
        let mut buffer: Vec<u8> = Vec::new();
        let mut file = tokio::fs::File::open(spans_file_path).await?;
        file.read_to_end(&mut buffer).await?;
        let bytes = BytesChunkReader {
            bytes: bytes::Bytes::from(buffer),
        };
        let file_reader = SerializedFileReader::new(bytes)?;
        let mut scopes = ScopeHashMap::new();
        for row in file_reader.get_row_iter(None)? {
            let hash = row.get_int(0)? as u32;
            let name = row.get_bytes(1)?;
            let filename = row.get_bytes(2)?;
            let line = row.get_int(3)? as u32;
            scopes.insert(
                hash,
                ScopeDesc {
                    name: String::from_utf8_lossy(name.data()).to_string(),
                    filename: String::from_utf8_lossy(filename.data()).to_string(),
                    line,
                    hash,
                },
            );
        }
        Ok(scopes)
    }

    async fn read_thread_block(
        &self,
        block_id: &str,
        spans_file_path: &Path,
        scopes_file_path: &Path,
    ) -> Result<BlockSpansReply> {
        let lod = self.read_spans_lod0(spans_file_path).await?;
        let scopes = self.read_scopes(scopes_file_path).await?;
        let (min_begin, max_end) = if !lod.tracks.is_empty() && !lod.tracks[0].spans.is_empty() {
            (
                lod.tracks[0].spans[0].begin_ms,
                lod.tracks[0].spans[lod.tracks[0].spans.len() - 1].end_ms,
            )
        } else {
            (f64::MAX, f64::MIN)
        };
        Ok(BlockSpansReply {
            scopes,
            lod: Some(lod),
            block_id: block_id.to_owned(),
            begin_ms: min_begin,
            end_ms: max_end,
        })
    }

    async fn read_tree_from_parquet(&self, spans_file_path: &Path) -> Result<TabularSpanTree> {
        let mut buffer: Vec<u8> = Vec::new();
        let mut file = tokio::fs::File::open(spans_file_path).await?;
        file.read_to_end(&mut buffer).await?;
        let bytes = BytesChunkReader {
            bytes: bytes::Bytes::from(buffer),
        };
        let file_reader = SerializedFileReader::new(bytes)?;
        build_span_tree(&file_reader)
    }

    async fn read_tree_block(
        &self,
        spans_file_path: &Path,
        scopes_file_path: &Path,
    ) -> Result<(ScopeHashMap, TabularSpanTree)> {
        let scopes = self.read_scopes(scopes_file_path).await?;
        let tree = self.read_tree_from_parquet(spans_file_path).await?;
        Ok((scopes, tree))
    }

    fn get_table_files(
        &self,
        process: &telemetry_sink::ProcessInfo,
        block_id: &str,
    ) -> (PathBuf, PathBuf) {
        let spans_file_path = self
            .tables_path
            .join("spans")
            .join(format!("process_id={}", &process.process_id))
            .join(format!("block_id={}", block_id))
            .join("spans.parquet");

        let scopes_file_path = self
            .tables_path
            .join("scopes")
            .join(format!("process_id={}", &process.process_id))
            .join(format!("block_id={}", block_id))
            .join("scopes.parquet");
        (spans_file_path, scopes_file_path)
    }
}

#[async_trait]
impl JitLakehouse for LocalJitLakehouse {
    #[cfg(feature = "deltalake-proto")]
    async fn build_timeline_tables(&self, process_id: &str) -> Result<()> {
        async_span_scope!("build_timeline_tables");
        let mut connection = self.pool.acquire().await?;
        let process = lgn_analytics::find_process(&mut connection, process_id).await?;
        let convert_ticks = ConvertTicks::new(&process);
        let spans_table_path = self
            .tables_path
            .join("spans")
            .join(format!("process_id={}", process_id));
        tokio::fs::create_dir_all(&spans_table_path)
            .await
            .with_context(|| format!("creating folder {}", spans_table_path.display()))?;

        update_spans_delta_table(
            self.pool.clone(),
            self.blob_storage.clone(),
            process_id,
            &convert_ticks,
            spans_table_path,
        )
        .await?;
        Ok(())
    }

    async fn get_thread_block(
        &self,
        process: &telemetry_sink::ProcessInfo,
        stream: &telemetry_sink::stream_info::StreamInfo,
        block_id: &str,
    ) -> Result<BlockSpansReply> {
        let (spans_file_path, scopes_file_path) = self.get_table_files(process, block_id);
        if !spans_file_path.exists() || !scopes_file_path.exists() {
            let (scopes, tree) = self
                .write_call_tree(process, stream, block_id, spans_file_path, scopes_file_path)
                .await?;
            if tree.is_empty() {
                return Ok(BlockSpansReply {
                    scopes: ScopeHashMap::new(),
                    lod: Some(SpanBlockLod {
                        lod_id: 0,
                        tracks: vec![],
                    }),
                    block_id: block_id.to_owned(),
                    begin_ms: f64::MAX,
                    end_ms: f64::MIN,
                });
            }
            let lod = lod0_from_span_tree(&tree)?;
            let root_span = tree.get_row(0)?;
            return Ok(BlockSpansReply {
                scopes,
                lod: Some(lod),
                block_id: block_id.to_owned(),
                begin_ms: root_span.begin_ms,
                end_ms: root_span.end_ms,
            });
        }

        self.read_thread_block(block_id, &spans_file_path, &scopes_file_path)
            .await
    }

    async fn get_call_tree(
        &self,
        process: &telemetry_sink::ProcessInfo,
        stream: &telemetry_sink::stream_info::StreamInfo,
        block_id: &str,
    ) -> Result<(ScopeHashMap, TabularSpanTree)> {
        let (spans_file_path, scopes_file_path) = self.get_table_files(process, block_id);
        if !spans_file_path.exists() || !scopes_file_path.exists() {
            return self
                .write_call_tree(process, stream, block_id, spans_file_path, scopes_file_path)
                .await;
        }

        self.read_tree_block(&spans_file_path, &scopes_file_path)
            .await
    }
}
