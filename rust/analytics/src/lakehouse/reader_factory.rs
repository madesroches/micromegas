use super::partition::Partition;
use anyhow::Result;
use bytes::Bytes;
use datafusion::{
    datasource::physical_plan::{FileMeta, ParquetFileReaderFactory},
    parquet::{
        arrow::{
            arrow_reader::ArrowReaderOptions,
            async_reader::{AsyncFileReader, ParquetObjectReader},
        },
        file::metadata::ParquetMetaData,
    },
    physical_plan::metrics::ExecutionPlanMetricsSet,
};
use futures::FutureExt;
use futures::future::BoxFuture;
use micromegas_tracing::prelude::*;
use object_store::ObjectStore;
use std::ops::Range;
use std::sync::Arc;

/// A custom [`ParquetFileReaderFactory`] that handles opening parquet files
/// from object storage, and uses pre-loaded metadata.
#[derive(Debug)]
pub struct ReaderFactory {
    object_store: Arc<dyn ObjectStore>,
    partition_domain: Arc<Vec<Partition>>,
}

impl ReaderFactory {
    pub fn new(object_store: Arc<dyn ObjectStore>, partition_domain: Arc<Vec<Partition>>) -> Self {
        Self {
            object_store,
            partition_domain,
        }
    }
}

fn find_parquet_metadata(filename: &str, domain: &[Partition]) -> Result<Arc<ParquetMetaData>> {
    for part in domain {
        if part.file_path == filename {
            return Ok(part.file_metadata.clone());
        }
    }
    anyhow::bail!("[reader_factory] file not found {filename}")
}

impl ParquetFileReaderFactory for ReaderFactory {
    fn create_reader(
        &self,
        _partition_index: usize,
        file_meta: FileMeta,
        metadata_size_hint: Option<usize>,
        _metrics: &ExecutionPlanMetricsSet,
    ) -> datafusion::error::Result<Box<dyn AsyncFileReader + Send>> {
        // todo: don't ignore metrics, report performance of the reader
        let filename = file_meta.location().to_string();
        let object_store = Arc::clone(&self.object_store);
        let mut inner = ParquetObjectReader::new(object_store, file_meta.location().clone());
        if let Some(hint) = metadata_size_hint {
            inner = inner.with_footer_size_hint(hint)
        };
        let metadata = find_parquet_metadata(&filename, &self.partition_domain)
            .map_err(|e| datafusion::error::DataFusionError::External(e.into()))?;
        // debug!("create_reader filename={filename}");
        Ok(Box::new(ParquetReader {
            filename,
            metadata,
            inner,
        }))
    }
}

/// A wrapper around a `ParquetObjectReader` that caches metadata.
pub struct ParquetReader {
    pub filename: String,
    pub metadata: Arc<ParquetMetaData>,
    pub inner: ParquetObjectReader,
}

impl AsyncFileReader for ParquetReader {
    fn get_bytes(
        &mut self,
        range: Range<u64>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Bytes>> {
        // debug!("ParquetReader::get_bytes {}", &self.filename);
        self.inner.get_bytes(range)
    }

    fn get_byte_ranges(
        &mut self,
        ranges: Vec<Range<u64>>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Vec<Bytes>>> {
        // debug!("ParquetReader::get_byte_ranges {}", &self.filename);
        self.inner.get_byte_ranges(ranges)
    }

    fn get_metadata(
        &mut self,
        _options: Option<&ArrowReaderOptions>,
    ) -> BoxFuture<'_, datafusion::parquet::errors::Result<Arc<ParquetMetaData>>> {
        let metadata = self.metadata.clone();
        async move { Ok(metadata) }.boxed()
    }
}
