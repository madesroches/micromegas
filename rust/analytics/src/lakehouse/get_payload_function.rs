use async_trait::async_trait;
use datafusion::{
    arrow::{
        array::{Array, BinaryBuilder, StringArray},
        datatypes::DataType,
    },
    common::{internal_err, not_impl_err},
    error::DataFusionError,
    logical_expr::{
        ColumnarValue, ScalarFunctionArgs, ScalarUDFImpl, Signature, Volatility,
        async_udf::AsyncScalarUDFImpl,
    },
};
use futures::stream::StreamExt;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// A scalar UDF that retrieves the payload of a block from the data lake.
#[derive(Debug)]
pub struct GetPayload {
    signature: Signature,
    lake: Arc<DataLakeConnection>,
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
    pub fn new(lake: Arc<DataLakeConnection>) -> Self {
        Self {
            signature: Signature::exact(
                vec![DataType::Utf8, DataType::Utf8, DataType::Utf8],
                Volatility::Immutable,
            ),
            lake,
        }
    }
}

impl ScalarUDFImpl for GetPayload {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

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
                DataFusionError::Execution("downcasting process_ids in GetPayload".into())
            })?
            .clone();
        let stream_ids = args[1]
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| {
                DataFusionError::Execution("downcasting stream_ids in GetPayload".into())
            })?
            .clone();
        let block_ids = args[2]
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| {
                DataFusionError::Execution("downcasting block_ids in GetPayload".into())
            })?
            .clone();
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
                    DataFusionError::Execution(format!("error downloading payload: {e:?}"))
                })?
                .map_err(|e| {
                    DataFusionError::Execution(format!("error downloading payload: {e:?}"))
                })?,
            );
        }
        Ok(ColumnarValue::Array(Arc::new(result_builder.finish())))
    }
}
