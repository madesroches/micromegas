//! `check_view_merge(ddl)` -- admin UDTF that plans the merge of a `CREATE [OR REPLACE]
//! MATERIALIZED VIEW` definition exactly as the maintenance daemon would (without executing it)
//! and reports whether a `SortExec` remains, i.e. whether the merge loses its streaming per-file
//! k-way shape and buffers its whole input. Yields no row for a view without `merge_sort_order`.
//! Registered inside the same admin-gated block as `list_view_set_definitions()`.

use super::{
    lakehouse_context::LakehouseContext,
    session_configurator::SessionConfigurator,
    view_ddl::{ViewDdl, parse_view_ddl},
    view_definition::ViewDefinition,
    view_factory::ViewFactory,
    view_merge_check::{MergeStreamingReport, check_merge_streaming},
};
use crate::dfext::expressions::exp_to_string;
use async_trait::async_trait;
use datafusion::arrow::array::{BooleanArray, RecordBatch, StringArray};
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::catalog::Session;
use datafusion::catalog::TableFunctionArgs;
use datafusion::catalog::TableFunctionImpl;
use datafusion::catalog::TableProvider;
use datafusion::common::plan_err;
use datafusion::datasource::TableType;
use datafusion::datasource::memory::{DataSourceExec, MemorySourceConfig};
use datafusion::error::DataFusionError;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::prelude::Expr;
use std::sync::Arc;

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("view_set_name", DataType::Utf8, false),
        Field::new("merge_sort_order", DataType::Utf8, false),
        Field::new("streams", DataType::Boolean, false),
        Field::new("sort_exec_line", DataType::Utf8, true),
        Field::new("plan", DataType::Utf8, false),
    ]))
}

/// Builds the output batch: one row per report, none when the check was skipped.
fn build_batch(report: Option<&MergeStreamingReport>) -> datafusion::error::Result<RecordBatch> {
    let reports: &[&MergeStreamingReport] = match &report {
        Some(r) => &[r],
        None => &[],
    };
    Ok(RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(StringArray::from_iter_values(
                reports.iter().map(|r| r.view_set_name.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                reports.iter().map(|r| r.merge_sort_order.join(", ")),
            )),
            Arc::new(BooleanArray::from_iter(
                reports.iter().map(|r| Some(r.streams)),
            )),
            Arc::new(StringArray::from_iter(
                reports.iter().map(|r| r.sort_exec_line.as_deref()),
            )),
            Arc::new(StringArray::from_iter_values(
                reports.iter().map(|r| r.plan.as_str()),
            )),
        ],
    )?)
}

/// A DataFusion `TableFunctionImpl` for `check_view_merge(ddl)`.
#[derive(Debug)]
pub struct CheckViewMergeTableFunction {
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    session_configurator: Arc<dyn SessionConfigurator>,
}

impl CheckViewMergeTableFunction {
    pub fn new(
        lakehouse: Arc<LakehouseContext>,
        view_factory: Arc<ViewFactory>,
        session_configurator: Arc<dyn SessionConfigurator>,
    ) -> Self {
        Self {
            lakehouse,
            view_factory,
            session_configurator,
        }
    }
}

impl TableFunctionImpl for CheckViewMergeTableFunction {
    fn call_with_args(
        &self,
        args: TableFunctionArgs,
    ) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        let Some(ddl_text) = args.exprs().first().map(exp_to_string).transpose()? else {
            return plan_err!("Missing first argument, expected ddl: String");
        };
        let definition = match parse_view_ddl(&ddl_text)
            .map_err(|e| DataFusionError::Plan(format!("check_view_merge: {e:#}")))?
        {
            Some(ViewDdl::Create { definition, .. }) => *definition,
            Some(ViewDdl::Drop { .. }) => {
                return plan_err!("check_view_merge: expected CREATE MATERIALIZED VIEW, got DROP");
            }
            None => {
                return plan_err!(
                    "check_view_merge: expected a CREATE [OR REPLACE] MATERIALIZED VIEW statement"
                );
            }
        };
        Ok(Arc::new(CheckViewMergeTableProvider {
            definition,
            lakehouse: self.lakehouse.clone(),
            view_factory: self.view_factory.clone(),
            session_configurator: self.session_configurator.clone(),
        }))
    }
}

#[derive(Debug)]
struct CheckViewMergeTableProvider {
    definition: ViewDefinition,
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    session_configurator: Arc<dyn SessionConfigurator>,
}

#[async_trait]
impl TableProvider for CheckViewMergeTableProvider {
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
        _limit: Option<usize>,
    ) -> datafusion::error::Result<Arc<dyn ExecutionPlan>> {
        let report = check_merge_streaming(
            &self.definition,
            self.lakehouse.clone(),
            self.view_factory.clone(),
            self.session_configurator.clone(),
        )
        .await
        .map_err(|e| DataFusionError::Execution(format!("check_view_merge: {e:#}")))?;
        let rb = build_batch(report.as_ref())?;
        let source =
            MemorySourceConfig::try_new(&[vec![rb]], schema(), projection.map(|v| v.to_owned()))?;
        Ok(DataSourceExec::from_data_source(source))
    }
}
