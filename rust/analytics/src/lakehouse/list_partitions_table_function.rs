use crate::sql_arrow_bridge::rows_to_record_batch;
use async_trait::async_trait;
use datafusion::arrow::datatypes::DataType;
use datafusion::arrow::datatypes::Field;
use datafusion::arrow::datatypes::Schema;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::arrow::datatypes::TimeUnit;
use datafusion::catalog::Session;
use datafusion::catalog::TableFunctionImpl;
use datafusion::catalog::TableProvider;
use datafusion::datasource::TableType;
use datafusion::error::DataFusionError;
use datafusion::physical_plan::memory::MemoryExec;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::prelude::Expr;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use std::any::Any;
use std::sync::Arc;

#[derive(Debug)]
pub struct ListPartitionsTableFunction {
    lake: Arc<DataLakeConnection>,
}

impl ListPartitionsTableFunction {
    pub fn new(lake: Arc<DataLakeConnection>) -> Self {
        Self { lake }
    }
}

impl TableFunctionImpl for ListPartitionsTableFunction {
    fn call(
        &self,
        _args: &[datafusion::prelude::Expr],
    ) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        Ok(Arc::new(ListPartitionsTableProvider {
            lake: self.lake.clone(),
        }))
    }
}

#[derive(Debug)]
pub struct ListPartitionsTableProvider {
    pub lake: Arc<DataLakeConnection>,
}

#[async_trait]
impl TableProvider for ListPartitionsTableProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("view_set_name", DataType::Utf8, false),
            Field::new("view_instance_id", DataType::Utf8, false),
            Field::new(
                "begin_insert_time",
                DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
                false,
            ),
            Field::new(
                "end_insert_time",
                DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
                false,
            ),
            Field::new(
                "min_event_time",
                DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
                false,
            ),
            Field::new(
                "max_event_time",
                DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
                false,
            ),
            Field::new(
                "updated",
                DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
                false,
            ),
            Field::new("file_path", DataType::Utf8, false),
            Field::new("file_size", DataType::Int64, false),
            Field::new("file_schema_hash", DataType::Binary, false),
            Field::new("source_data_hash", DataType::Binary, false),
        ]))
    }

    fn table_type(&self) -> TableType {
        TableType::Temporary
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        _limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        let rows = sqlx::query(
            "SELECT view_set_name,
				    view_instance_id,
				    begin_insert_time,
				    end_insert_time,
				    min_event_time,
				    max_event_time,
				    updated, file_path,
				    file_size,
				    file_schema_hash,
				    source_data_hash
			 FROM lakehouse_partitions;",
        )
        .fetch_all(&self.lake.db_pool)
        .await
        .map_err(|e| DataFusionError::External(e.into()))?;
        let rb = rows_to_record_batch(&rows).map_err(|e| DataFusionError::External(e.into()))?;
        Ok(Arc::new(MemoryExec::try_new(
            &[vec![rb]],
            self.schema(),
            projection.map(|v| v.to_owned()),
        )?))
    }
}
