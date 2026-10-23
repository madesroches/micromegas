//! `list_view_set_definitions()` -- admin UDTF listing every view set: the rows of
//! `lakehouse_view_set_definitions` read straight from Postgres (`source = 'ddl'`), plus the
//! built-in view sets of the `ViewFactory` that have no such row (`source = 'builtin'`).
//! Postgres rows are read directly rather than from `ViewRegistry::current()` so an admin can
//! see a definition that exists on disk but failed to load. Built-in rows carry the
//! `update_group` of their global view, which gives clients the materialization order.
//! Registered inside the same admin-gated block as `list_query_denials()`'s siblings.

use super::view_factory::ViewFactory;
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
use std::collections::HashSet;
use std::sync::Arc;

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("view_set_name", DataType::Utf8, false),
        Field::new("definition_sql", DataType::Utf8, true),
        Field::new("update_group", DataType::Int32, true),
        Field::new(
            "updated_at",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            true,
        ),
        Field::new("updated_by", DataType::Utf8, true),
        Field::new("source", DataType::Utf8, false),
    ]))
}

/// A row of `lakehouse_view_set_definitions`.
#[derive(Debug, Clone)]
pub struct DdlRow {
    pub view_set_name: String,
    pub definition_sql: String,
    pub update_group: i32,
    pub updated_at_nanos: i64,
    pub updated_by: Option<String>,
}

/// A row of the `list_view_set_definitions()` output.
#[derive(Debug, Clone, PartialEq)]
pub struct DefinitionRow {
    pub view_set_name: String,
    pub definition_sql: Option<String>,
    pub update_group: Option<i32>,
    pub updated_at_nanos: Option<i64>,
    pub updated_by: Option<String>,
    pub source: &'static str,
}

/// Unions the Postgres rows with the factory view sets they do not name, sorted by name.
/// A Postgres row wins on a name collision, which covers a DDL set that is also in the factory
/// and one that failed to load.
pub fn merge_rows(ddl: Vec<DdlRow>, factory: &ViewFactory) -> Vec<DefinitionRow> {
    let mut rows: Vec<DefinitionRow> = ddl
        .into_iter()
        .map(|r| DefinitionRow {
            view_set_name: r.view_set_name,
            definition_sql: Some(r.definition_sql),
            update_group: Some(r.update_group),
            updated_at_nanos: Some(r.updated_at_nanos),
            updated_by: r.updated_by,
            source: "ddl",
        })
        .collect();
    let mut seen: HashSet<String> = rows.iter().map(|r| r.view_set_name.clone()).collect();
    let mut builtin = |name: String, update_group: Option<i32>| {
        if seen.insert(name.clone()) {
            rows.push(DefinitionRow {
                view_set_name: name,
                definition_sql: None,
                update_group,
                updated_at_nanos: None,
                updated_by: None,
                source: "builtin",
            });
        }
    };
    for view in factory.get_global_views() {
        builtin(
            view.get_view_set_name().to_string(),
            view.get_update_group(),
        );
    }
    for name in factory.get_view_sets().keys() {
        builtin(name.clone(), None);
    }
    rows.sort_by(|a, b| a.view_set_name.cmp(&b.view_set_name));
    rows
}

/// Builds the output batch, keeping at most `limit` rows.
pub fn build_batch(
    rows: &[DefinitionRow],
    limit: Option<usize>,
) -> datafusion::error::Result<RecordBatch> {
    let rows = &rows[..limit.map_or(rows.len(), |n| n.min(rows.len()))];
    Ok(RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.view_set_name.as_str()),
            )),
            Arc::new(StringArray::from_iter(
                rows.iter().map(|r| r.definition_sql.as_deref()),
            )),
            Arc::new(Int32Array::from_iter(rows.iter().map(|r| r.update_group))),
            Arc::new(
                TimestampNanosecondArray::from_iter(rows.iter().map(|r| r.updated_at_nanos))
                    .with_timezone("+00:00".to_string()),
            ),
            Arc::new(StringArray::from_iter(
                rows.iter().map(|r| r.updated_by.as_deref()),
            )),
            Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.source))),
        ],
    )?)
}

/// A DataFusion `TableFunctionImpl` for `list_view_set_definitions()`.
#[derive(Debug)]
pub struct ListViewSetDefinitionsTableFunction {
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
}

impl ListViewSetDefinitionsTableFunction {
    pub fn new(lake: Arc<DataLakeConnection>, view_factory: Arc<ViewFactory>) -> Self {
        Self { lake, view_factory }
    }
}

impl TableFunctionImpl for ListViewSetDefinitionsTableFunction {
    fn call_with_args(
        &self,
        _args: TableFunctionArgs,
    ) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        Ok(Arc::new(ListViewSetDefinitionsTableProvider {
            lake: self.lake.clone(),
            view_factory: self.view_factory.clone(),
        }))
    }
}

#[derive(Debug)]
struct ListViewSetDefinitionsTableProvider {
    lake: Arc<DataLakeConnection>,
    view_factory: Arc<ViewFactory>,
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
        let mut ddl = Vec::with_capacity(rows.len());
        for row in &rows {
            let ts: chrono::DateTime<chrono::Utc> = row
                .try_get("updated_at")
                .map_err(|e| DataFusionError::External(e.into()))?;
            ddl.push(DdlRow {
                view_set_name: row
                    .try_get("view_set_name")
                    .map_err(|e| DataFusionError::External(e.into()))?,
                definition_sql: row
                    .try_get("definition_sql")
                    .map_err(|e| DataFusionError::External(e.into()))?,
                update_group: row
                    .try_get("update_group")
                    .map_err(|e| DataFusionError::External(e.into()))?,
                updated_at_nanos: ts.timestamp_nanos_opt().unwrap_or_default(),
                updated_by: row
                    .try_get("updated_by")
                    .map_err(|e| DataFusionError::External(e.into()))?,
            });
        }
        let rb = build_batch(&merge_rows(ddl, &self.view_factory), limit)?;
        let source =
            MemorySourceConfig::try_new(&[vec![rb]], schema(), projection.map(|v| v.to_owned()))?;
        Ok(DataSourceExec::from_data_source(source))
    }
}
