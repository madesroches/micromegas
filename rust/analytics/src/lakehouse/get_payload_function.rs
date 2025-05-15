use datafusion::{
    arrow::{
        array::{Array, BinaryBuilder, StringArray},
        datatypes::DataType,
    },
    common::internal_err,
    error::DataFusionError,
    logical_expr::{ColumnarValue, ScalarUDFImpl, Signature, Volatility},
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::sync::Arc;

#[derive(Debug)]
pub struct GetPayload {
    signature: Signature,
    lake: Arc<DataLakeConnection>,
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
        args: datafusion::logical_expr::ScalarFunctionArgs,
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
        std::thread::spawn(|| {
            // hack until DataFusion supports async user defined scalar functions
            // https://github.com/apache/datafusion/pull/14837
            let rt =
                tokio::runtime::Runtime::new().map_err(|e| DataFusionError::External(e.into()))?;
            rt.block_on(fetch_blocks(lake, process_ids, stream_ids, block_ids))
        })
        .join()
        .expect("Thread panicked")
    }
}

async fn fetch_blocks(
    lake: Arc<DataLakeConnection>,
    process_ids: StringArray,
    stream_ids: StringArray,
    block_ids: StringArray,
) -> datafusion::error::Result<ColumnarValue> {
    let mut result_builder = BinaryBuilder::with_capacity(block_ids.len(), 1024 * 1024);
    for i in 0..process_ids.len() {
        let process_id = process_ids.value(i);
        let stream_id = stream_ids.value(i);
        let block_id = block_ids.value(i);
        let obj_path = format!("blobs/{process_id}/{stream_id}/{block_id}");
        let buffer = lake
            .blob_storage
            .read_blob(&obj_path)
            .await
            .map_err(|e| DataFusionError::External(e.into()))?;
        result_builder.append_value(buffer);
    }
    Ok(ColumnarValue::Array(Arc::new(result_builder.finish())))
}
