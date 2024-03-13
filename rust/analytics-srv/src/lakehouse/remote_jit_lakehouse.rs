use crate::{
    call_tree::process_thread_block,
    lakehouse::{
        bytes_chunk_reader::BytesChunkReader, jit_lakehouse::JitLakehouse,
        span_table::make_rows_from_tree,
    },
    scope::ScopeHashMap,
};
use micromegas_analytics::time::ConvertTicks;
use anyhow::{Context, Result};
use async_trait::async_trait;
use lgn_blob_storage::{AwsS3Url, BlobStorage};
use lgn_telemetry_proto::analytics::{BlockSpansReply, ScopeDesc, SpanBlockLod};
use parquet::file::serialized_reader::SerializedFileReader;
use parquet::{file::reader::FileReader, record::RowAccessor};
use sqlx::PgPool;
use std::sync::Arc;
use micromegas_tracing::prelude::*;

use super::{
    scope_table::{make_scopes_table_writer, ScopeRowGroup},
    span_table::{
        build_span_tree, build_spans_lod0, lod0_from_span_tree, make_spans_table_writer,
        SpanRowGroup, TabularSpanTree,
    },
};

pub struct RemoteJitLakehouse {
    pool: PgPool,
    blob_storage: Arc<dyn BlobStorage>,
    tables_uri: AwsS3Url,
    s3client: aws_sdk_s3::Client,
}

impl RemoteJitLakehouse {
    pub async fn new(
        pool: PgPool,
        blob_storage: Arc<dyn BlobStorage>,
        tables_uri: AwsS3Url,
    ) -> Self {
        let config = aws_config::load_from_env().await;
        let s3client = aws_sdk_s3::Client::new(&config);

        Self {
            pool,
            blob_storage,
            tables_uri,
            s3client,
        }
    }

    async fn read_spans_lod0(&self, spans_key: String) -> Result<SpanBlockLod> {
        let get_obj_output = self
            .s3client
            .get_object()
            .bucket(&self.tables_uri.bucket_name)
            .key(&spans_key)
            .send()
            .await?;
        let bytes = BytesChunkReader {
            bytes: get_obj_output.body.collect().await?.into_bytes(),
        };
        let file_reader = SerializedFileReader::new(bytes)?;
        build_spans_lod0(&file_reader)
    }

    async fn read_tree(&self, spans_key: String) -> Result<TabularSpanTree> {
        //todo: factor out creation of file reader from key
        let get_obj_output = self
            .s3client
            .get_object()
            .bucket(&self.tables_uri.bucket_name)
            .key(&spans_key)
            .send()
            .await?;
        let bytes = BytesChunkReader {
            bytes: get_obj_output.body.collect().await?.into_bytes(),
        };
        let file_reader = SerializedFileReader::new(bytes)?;
        build_span_tree(&file_reader)
    }

    async fn read_scopes(&self, scopes_key: String) -> Result<ScopeHashMap> {
        let get_obj_output = self
            .s3client
            .get_object()
            .bucket(&self.tables_uri.bucket_name)
            .key(&scopes_key)
            .send()
            .await?;
        let bytes = BytesChunkReader {
            bytes: get_obj_output.body.collect().await?.into_bytes(),
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
        spans_key: String,
        scopes_key: String,
    ) -> Result<BlockSpansReply> {
        let lod = self.read_spans_lod0(spans_key).await?;
        let scopes = self.read_scopes(scopes_key).await?;
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

    async fn write_scopes(&self, scopes: &ScopeHashMap, key: String) -> Result<()> {
        let mut rows = ScopeRowGroup::new();
        for (_k, v) in scopes.iter() {
            rows.append(v);
        }
        let mut writer = make_scopes_table_writer()?;
        writer.write_row_group(&rows.get_columns())?;
        let buffer = Arc::get_mut(&mut writer.close()?)
            .with_context(|| "getting exclusive access to parquet buffer")?
            .clone();
        let body = aws_sdk_s3::types::ByteStream::from(buffer.into_inner());
        self.s3client
            .put_object()
            .bucket(&self.tables_uri.bucket_name)
            .key(key)
            .body(body)
            .send()
            .await?;
        Ok(())
    }

    async fn write_spans(&self, rows: &SpanRowGroup, key: String) -> Result<()> {
        let mut writer = make_spans_table_writer()?;
        writer.write_row_group(&rows.get_columns())?;
        let buffer = Arc::get_mut(&mut writer.close()?)
            .with_context(|| "getting exclusive access to parquet buffer")?
            .clone();
        let body = aws_sdk_s3::types::ByteStream::from(buffer.into_inner());
        self.s3client
            .put_object()
            .bucket(&self.tables_uri.bucket_name)
            .key(key)
            .body(body)
            .send()
            .await?;
        Ok(())
    }

    async fn write_call_tree(
        &self,
        process: &micromegas_telemetry_sink::ProcessInfo,
        stream: &micromegas_telemetry_sink::stream_info::StreamInfo,
        block_id: &str,
        spans_key: String,
        scopes_key: String,
    ) -> Result<(ScopeHashMap, TabularSpanTree)> {
        info!("writing thread block {}", block_id);
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
        self.write_spans(&rows, spans_key).await?;
        self.write_scopes(&processed.scopes, scopes_key).await?;

        Ok((processed.scopes, TabularSpanTree::from_rows(&rows)?))
    }

    async fn object_exists(&self, key: &str) -> Result<bool> {
        let req = self
            .s3client
            .head_object()
            .bucket(&self.tables_uri.bucket_name)
            .key(key);

        match req.send().await {
            Ok(_output) => {
                //dbg!(output);
                Ok(true)
            }
            Err(aws_sdk_s3::types::SdkError::ServiceError { err, raw: _ }) => {
                if let aws_sdk_s3::error::HeadObjectErrorKind::NotFound(_) = err.kind {
                    Ok(false)
                } else {
                    anyhow::bail!(err)
                }
            }
            Err(err) => anyhow::bail!(err),
        }
    }

    async fn read_tree_block(
        &self,
        spans_key: String,
        scopes_key: String,
    ) -> Result<(ScopeHashMap, TabularSpanTree)> {
        let scopes = self.read_scopes(scopes_key).await?;
        let tree = self.read_tree(spans_key).await?;
        Ok((scopes, tree))
    }

    fn get_table_keys(
        &self,
        process: &micromegas_telemetry_sink::ProcessInfo,
        block_id: &str,
    ) -> (String, String) {
        let spans_key = format!(
            "{}spans/process_id={}/block_id={}/spans.parquet",
            &self.tables_uri.root, &process.process_id, block_id
        );
        let scopes_key = format!(
            "{}scopes/process_id={}/block_id={}/scopes.parquet",
            &self.tables_uri.root, &process.process_id, block_id
        );
        (spans_key, scopes_key)
    }
}

#[async_trait]
impl JitLakehouse for RemoteJitLakehouse {
    #[cfg(feature = "deltalake-proto")]
    async fn build_timeline_tables(&self, _process_id: &str) -> Result<()> {
        //not implemented
        Ok(())
    }

    async fn get_thread_block(
        &self,
        process: &micromegas_telemetry_sink::ProcessInfo,
        stream: &micromegas_telemetry_sink::stream_info::StreamInfo,
        block_id: &str,
    ) -> Result<BlockSpansReply> {
        let (spans_key, scopes_key) = self.get_table_keys(process, block_id);
        if !self.object_exists(&spans_key).await? || !self.object_exists(&scopes_key).await? {
            let (scopes, tree) = self
                .write_call_tree(process, stream, block_id, spans_key, scopes_key)
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

        self.read_thread_block(block_id, spans_key, scopes_key)
            .await
    }

    async fn get_call_tree(
        &self,
        process: &micromegas_telemetry_sink::ProcessInfo,
        stream: &micromegas_telemetry_sink::stream_info::StreamInfo,
        block_id: &str,
    ) -> Result<(ScopeHashMap, TabularSpanTree)> {
        let (spans_key, scopes_key) = self.get_table_keys(process, block_id);
        if !self.object_exists(&spans_key).await? || !self.object_exists(&scopes_key).await? {
            return self
                .write_call_tree(process, stream, block_id, spans_key, scopes_key)
                .await;
        }
        self.read_tree_block(spans_key, scopes_key).await
    }
}
