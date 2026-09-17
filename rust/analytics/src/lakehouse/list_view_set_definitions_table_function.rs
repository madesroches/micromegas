//! `list_view_set_definitions()` -- admin UDTF listing every row of `lakehouse_view_set_definitions`,
//! straight from Postgres rather than from `ViewRegistry::current()` -- so an admin can see a
//! definition that exists on disk but failed to load (present here, absent from
//! `list_view_sets()`). Registered inside the same admin-gated block as `list_query_denials()`'s
//! siblings.

use async_trait::async_trait;
use datafusion::arrow::array::{Int32Array, RecordBatch, StringArray, TimestampNanosecondArray};
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use datafusion::catalog::Session;
use datafusion::catalog::TableFunctionArgs;
use datafusion::catalog::TableFunctionImpl;
use datafusion::catalog::TableProvider;
use datafusion::datasource::TableType;
use datafusion::datasource::memory::{DataSourceExec, MemorySourceConfig};
use datafusion::error::DataFusionError;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::prelude::Expr;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use sqlx::Row;
use std::sync::Arc;

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("view_set_name", DataType::Utf8, false),
        Field::new("definition_sql", DataType::Utf8, false),
        Field::new("update_group", DataType::Int32, false),
        Field::new(
            "updated_at",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("updated_by", DataType::Utf8, true),
    ]))
}

/// A DataFusion `TableFunctionImpl` for `list_view_set_definitions()`.
#[derive(Debug)]
pub struct ListViewSetDefinitionsTableFunction {
    lake: Arc<DataLakeConnection>,
}

impl ListViewSetDefinitionsTableFunction {
    pub fn new(lake: Arc<DataLakeConnection>) -> Self {
        Self { lake }
    }
}

impl TableFunctionImpl for ListViewSetDefinitionsTableFunction {
    fn call_with_args(
        &self,
        _args: TableFunctionArgs,
    ) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        Ok(Arc::new(ListViewSetDefinitionsTableProvider {
            lake: self.lake.clone(),
        }))
    }
}

#[derive(Debug)]
struct ListViewSetDefinitionsTableProvider {
    lake: Arc<DataLakeConnection>,
}

#[async_trait]
impl TableProvider for ListViewSetDefinitionsTableProvider {
    fn schema(&self) -> SchemaRef {
        schema()
    }

    fn table_type(&self) -> TableType {
        TableType::Temporary
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        let rows = sqlx::query(
            "SELECT view_set_name, definition_sql, update_group, updated_at, updated_by \
             FROM lakehouse_view_set_definitions ORDER BY view_set_name",
        )
        .fetch_all(&self.lake.db_pool)
        .await
        .map_err(|e| DataFusionError::External(e.into()))?;
        let mut view_set_name = Vec::with_capacity(rows.len());
        let mut definition_sql = Vec::with_capacity(rows.len());
        let mut update_group = Vec::with_capacity(rows.len());
        let mut updated_at = Vec::with_capacity(rows.len());
        let mut updated_by = Vec::with_capacity(rows.len());
        for row in &rows {
            view_set_name.push(
                row.try_get::<String, _>("view_set_name")
                    .map_err(|e| DataFusionError::External(e.into()))?,
            );
            definition_sql.push(
                row.try_get::<String, _>("definition_sql")
                    .map_err(|e| DataFusionError::External(e.into()))?,
            );
            update_group.push(
                row.try_get::<i32, _>("update_group")
                    .map_err(|e| DataFusionError::External(e.into()))?,
            );
            let ts: chrono::DateTime<chrono::Utc> = row
                .try_get("updated_at")
                .map_err(|e| DataFusionError::External(e.into()))?;
            updated_at.push(ts.timestamp_nanos_opt().unwrap_or_default());
            updated_by.push(
                row.try_get::<Option<String>, _>("updated_by")
                    .map_err(|e| DataFusionError::External(e.into()))?,
            );
        }
        if let Some(n) = limit {
            view_set_name.truncate(n);
            definition_sql.truncate(n);
            update_group.truncate(n);
            updated_at.truncate(n);
            updated_by.truncate(n);
        }
        let rb = RecordBatch::try_new(
            schema(),
            vec![
                Arc::new(StringArray::from(view_set_name)),
                Arc::new(StringArray::from(definition_sql)),
                Arc::new(Int32Array::from(update_group)),
                Arc::new(
                    TimestampNanosecondArray::from(updated_at).with_timezone("+00:00".to_string()),
                ),
                Arc::new(StringArray::from(updated_by)),
            ],
        )?;
        let source =
            MemorySourceConfig::try_new(&[vec![rb]], schema(), projection.map(|v| v.to_owned()))?;
        Ok(DataSourceExec::from_data_source(source))
    }
}
