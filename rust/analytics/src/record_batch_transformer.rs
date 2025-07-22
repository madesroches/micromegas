use anyhow::Result;
use async_trait::async_trait;
use datafusion::arrow::array::RecordBatch;
use std::fmt::Debug;

#[async_trait]
pub trait RecordBatchTransformer: Send + Sync + Debug {
    async fn transform(&self, src: RecordBatch) -> Result<RecordBatch>;
}

#[derive(Debug)]
pub struct TrivialRecordBatchTransformer {}

#[async_trait]
impl RecordBatchTransformer for TrivialRecordBatchTransformer {
    async fn transform(&self, src: RecordBatch) -> Result<RecordBatch> {
        Ok(src)
    }
}
