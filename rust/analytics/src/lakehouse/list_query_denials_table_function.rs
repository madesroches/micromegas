//! `list_query_denials()` -- admin UDTF listing every query-deny-list rule currently in force.
//! Registered inside the same admin-gated block as `list_partitions()`'s mutating siblings.

use super::query_deny_list::QueryDenyList;
use async_trait::async_trait;
use datafusion::arrow::array::{RecordBatch, StringArray, TimestampNanosecondArray};
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
use std::sync::Arc;

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("rule_id", DataType::Utf8, false),
        Field::new(
            "created_at",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("created_by", DataType::Utf8, false),
        Field::new("reason", DataType::Utf8, false),
        Field::new("match_expr", DataType::Utf8, false),
        Field::new(
            "last_hit_at",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            true,
        ),
    ]))
}

/// A DataFusion `TableFunctionImpl` for `list_query_denials()`.
#[derive(Debug)]
pub struct ListQueryDenialsTableFunction {
    query_denials: Arc<QueryDenyList>,
}

impl ListQueryDenialsTableFunction {
    pub fn new(query_denials: Arc<QueryDenyList>) -> Self {
        Self { query_denials }
    }
}

impl TableFunctionImpl for ListQueryDenialsTableFunction {
    fn call_with_args(
        &self,
        _args: TableFunctionArgs,
    ) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        Ok(Arc::new(ListQueryDenialsTableProvider {
            query_denials: self.query_denials.clone(),
        }))
    }
}

#[derive(Debug)]
struct ListQueryDenialsTableProvider {
    query_denials: Arc<QueryDenyList>,
}

#[async_trait]
impl TableProvider for ListQueryDenialsTableProvider {
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
        let mut rows = self
            .query_denials
            .list()
            .await
            .map_err(|e| DataFusionError::External(e.into()))?;
        if let Some(n) = limit {
            rows.truncate(n);
        }
        let rule_id: StringArray = rows.iter().map(|r| Some(r.rule_id.to_string())).collect();
        let created_at: TimestampNanosecondArray = rows
            .iter()
            .map(|r| r.created_at.timestamp_nanos_opt())
            .collect::<TimestampNanosecondArray>()
            .with_timezone("+00:00".to_string());
        let created_by: StringArray = rows.iter().map(|r| Some(r.created_by.clone())).collect();
        let reason: StringArray = rows.iter().map(|r| Some(r.reason.clone())).collect();
        let match_expr: StringArray = rows.iter().map(|r| Some(r.match_expr.clone())).collect();
        let last_hit_at: TimestampNanosecondArray = rows
            .iter()
            .map(|r| r.last_hit_at.and_then(|t| t.timestamp_nanos_opt()))
            .collect::<TimestampNanosecondArray>()
            .with_timezone("+00:00".to_string());
        let rb = RecordBatch::try_new(
            schema(),
            vec![
                Arc::new(rule_id),
                Arc::new(created_at),
                Arc::new(created_by),
                Arc::new(reason),
                Arc::new(match_expr),
                Arc::new(last_hit_at),
            ],
        )?;
        let source =
            MemorySourceConfig::try_new(&[vec![rb]], schema(), projection.map(|v| v.to_owned()))?;
        Ok(DataSourceExec::from_data_source(source))
    }
}
