use super::{
    lakehouse_context::LakehouseContext, materialized_view::MaterializedView,
    partition_cache::QueryPartitionProvider, view_factory::ViewFactory,
};
use crate::{dfext::expressions::exp_to_string, time::TimeRange};
use datafusion::{
    catalog::{TableFunctionImpl, TableProvider},
    common::plan_err,
    error::DataFusionError,
    logical_expr::Expr,
};
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// `ViewInstanceTableFunction` gives access to any view instance using a [ViewFactory].
///
/// ```python
/// # Python code showing the usage of `view_instance(view_set_name, view_instance_id)`
/// sql = """
/// SELECT *
/// FROM view_instance('thread_spans', '{stream_id}')
/// ;""".format(stream_id=stream_id)
/// df_spans = client.query(sql, begin_spans, end_spans)
/// ```
///
#[derive(Debug)]
pub struct ViewInstanceTableFunction {
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
}

impl ViewInstanceTableFunction {
    pub fn new(
        lakehouse: Arc<LakehouseContext>,
        view_factory: Arc<ViewFactory>,
        part_provider: Arc<dyn QueryPartitionProvider>,
        query_range: Option<TimeRange>,
    ) -> Self {
        Self {
            lakehouse,
            view_factory,
            part_provider,
            query_range,
        }
    }
}

impl TableFunctionImpl for ViewInstanceTableFunction {
    #[span_fn]
    fn call(&self, exprs: &[Expr]) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        let arg1 = exprs.first().map(exp_to_string);
        let Some(Ok(view_set_name)) = arg1 else {
            return plan_err!(
                "First argument to view_instance must be a string (the view set name), given {:?}",
                arg1
            );
        };
        let arg2 = exprs.get(1).map(exp_to_string);
        let Some(Ok(view_instance_id)) = arg2 else {
            return plan_err!(
                "Second argument to view_instance must be a string (the view instance id), given {:?}",
                arg2
            );
        };

        let view = self
            .view_factory
            .make_view(&view_set_name, &view_instance_id)
            .map_err(|e| DataFusionError::Plan(format!("error making view {e:?}")))?;

        Ok(Arc::new(MaterializedView::new(
            self.lakehouse.clone(),
            self.lakehouse.reader_factory().clone(),
            view,
            self.part_provider.clone(),
            self.query_range,
        )))
    }
}
