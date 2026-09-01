use super::audience_guard::{AudienceGuard, IdKind};
use async_trait::async_trait;
use datafusion::{
    arrow::{
        array::{Array, BinaryBuilder, StringArray},
        datatypes::DataType,
    },
    common::{internal_err, not_impl_err, plan_err},
    error::DataFusionError,
    logical_expr::{
        ColumnarValue, ScalarFunctionArgs, ScalarUDFImpl, Signature, Volatility,
        async_udf::AsyncScalarUDFImpl,
    },
};
use futures::stream::StreamExt;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

/// A scalar UDF that retrieves the payload of a block from the data lake.
#[derive(Debug)]
pub struct GetPayload {
    signature: Signature,
    lake: Arc<DataLakeConnection>,
    guard: Arc<AudienceGuard>,
}

impl PartialEq for GetPayload {
    fn eq(&self, other: &Self) -> bool {
        self.signature == other.signature
    }
}

impl Eq for GetPayload {}

impl std::hash::Hash for GetPayload {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.signature.hash(state);
    }
}

impl GetPayload {
    pub fn new(lake: Arc<DataLakeConnection>, guard: Arc<AudienceGuard>) -> Self {
        Self {
            signature: Signature::exact(
                vec![DataType::Utf8, DataType::Utf8, DataType::Utf8],
                Volatility::Immutable,
            ),
            lake,
            guard,
        }
    }
}

impl ScalarUDFImpl for GetPayload {
    fn name(&self) -> &str {
        "get_payload"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> datafusion::error::Result<DataType> {
        Ok(DataType::Binary)
    }

    fn invoke_with_args(
        &self,
        _args: datafusion::logical_expr::ScalarFunctionArgs,
    ) -> datafusion::error::Result<ColumnarValue> {
        not_impl_err!("GetPayload can only be called from async contexts")
    }
}

#[async_trait]
impl AsyncScalarUDFImpl for GetPayload {
    async fn invoke_async_with_args(
        &self,
        args: ScalarFunctionArgs,
    ) -> datafusion::error::Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 3 {
            return internal_err!("wrong number of arguments to get_payload()");
        }
        let process_ids = args[0]
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| {
                DataFusionError::Internal("downcasting process_ids in GetPayload".into())
            })?
            .clone();
        let stream_ids = args[1]
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| {
                DataFusionError::Internal("downcasting stream_ids in GetPayload".into())
            })?
            .clone();
        let block_ids = args[2]
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| DataFusionError::Internal("downcasting block_ids in GetPayload".into()))?
            .clone();

        // The call-level guard: `get_payload` reads `blobs/{process_id}/{stream_id}
        // /{block_id}` directly out of object storage, bypassing the lakehouse entirely -- arg 1
        // (`process_id`) is therefore the whole check, and a complete one: a caller who names a
        // readable process cannot reach another process's blob, because the foreign block simply
        // isn't under that prefix. Distinct ids only (one resolution per unique process, not per
        // row); a value that doesn't even parse as a UUID is denied the same way an unreadable
        // one is, since it's the one input that could otherwise escape the `blobs/{process_id}`
        // prefix the completeness argument above relies on. All-or-nothing over the batch, never
        // a per-row `NULL`: a partially-filtered binary column would be indistinguishable from a
        // missing payload.
        let mut distinct_process_ids: Vec<Uuid> = Vec::new();
        let mut seen: HashSet<Uuid> = HashSet::new();
        for i in 0..process_ids.len() {
            let raw = process_ids.value(i);
            let Ok(id) = Uuid::parse_str(raw) else {
                return plan_err!("get_payload: '{raw}' is not a valid process id");
            };
            if seen.insert(id) {
                distinct_process_ids.push(id);
            }
        }
        let readable = self
            .guard
            .readable_ids(&distinct_process_ids, IdKind::Process)
            .await?;
        for id in &distinct_process_ids {
            if !readable.contains(id) {
                return plan_err!("get_payload: process '{id}' not found or not accessible");
            }
        }

        let lake = self.lake.clone();
        let mut stream = futures::stream::iter(0..process_ids.len())
            .map(|i| {
                let process_id = process_ids.value(i);
                let stream_id = stream_ids.value(i);
                let block_id = block_ids.value(i);
                let obj_path = format!("blobs/{process_id}/{stream_id}/{block_id}");
                let lake = lake.clone();
                spawn_with_context(async move { lake.blob_storage.read_blob(&obj_path).await })
            })
            .buffered(10);
        let mut result_builder = BinaryBuilder::with_capacity(block_ids.len(), 1024 * 1024);
        while let Some(res) = stream.next().await {
            result_builder.append_value(
                res.map_err(|e| {
                    DataFusionError::Internal(format!("error downloading payload: {e:?}"))
                })?
                .map_err(|e| {
                    DataFusionError::Internal(format!("error downloading payload: {e:?}"))
                })?,
            );
        }
        Ok(ColumnarValue::Array(Arc::new(result_builder.finish())))
    }
}
