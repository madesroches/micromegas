use crate::{lakehouse::materialized_view::MaterializedView, time::TimeRange};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::utils::conjunction;
use datafusion::logical_expr::Filter;
use datafusion::{
    common::tree_node::Transformed, config::ConfigOptions, datasource::DefaultTableSource,
    logical_expr::LogicalPlan, optimizer::AnalyzerRule,
};
use std::sync::Arc;

#[derive(Debug)]
pub struct TableScanRewrite {
    query_range: TimeRange,
}

impl TableScanRewrite {
    pub fn new(query_range: TimeRange) -> Self {
        Self { query_range }
    }

    fn rewrite_plan(
        &self,
        plan: LogicalPlan,
        _options: &ConfigOptions,
    ) -> datafusion::error::Result<Transformed<LogicalPlan>> {
        if let LogicalPlan::TableScan(ts) = &plan {
            let table_source = ts
                .source
                .as_any()
                .downcast_ref::<DefaultTableSource>()
                .ok_or_else(|| {
                    DataFusionError::Execution(String::from(
                        "error casting table source as DefaultTableSource",
                    ))
                })?;
            let mat_view = table_source
                .table_provider
                .as_any()
                .downcast_ref::<MaterializedView>()
                .ok_or_else(|| {
                    DataFusionError::Execution(String::from(
                        "error casting table provider as MaterializedView",
                    ))
                })?;
            let view = mat_view.get_view();
            let filters = view
                .make_time_filter(self.query_range.begin, self.query_range.end)
                .map_err(|e| DataFusionError::External(e.into()))?;
            let pred = conjunction(filters).ok_or_else(|| {
                DataFusionError::Execution(String::from("error making a conjunction"))
            })?;
            let filter = Filter::try_new(pred, Arc::new(plan.clone()))?;
            Ok(Transformed::yes(LogicalPlan::Filter(filter)))
        } else {
            Ok(Transformed::no(plan))
        }
    }
}

impl AnalyzerRule for TableScanRewrite {
    fn name(&self) -> &str {
        "table_scan_rewrite"
    }

    fn analyze(
        &self,
        plan: LogicalPlan,
        options: &ConfigOptions,
    ) -> datafusion::error::Result<LogicalPlan> {
        plan.transform_up_with_subqueries(|plan| self.rewrite_plan(plan, options))
            .map(|res| res.data)
    }
}
