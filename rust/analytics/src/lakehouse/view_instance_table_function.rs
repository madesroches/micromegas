use super::{
    audience_guard::AudienceGuard, lakehouse_context::LakehouseContext,
    materialized_view::MaterializedView, partition_cache::QueryPartitionProvider,
    view_factory::ViewFactory,
};
use crate::{dfext::expressions::exp_to_string, time::TimeRange};
use datafusion::{
    catalog::{TableFunctionArgs, TableFunctionImpl, TableProvider},
    common::plan_err,
    error::DataFusionError,
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
/// The only site that supplies an [`AudienceGuard`] to the [`MaterializedView`]s it builds:
/// every caller-named instance goes through this table function, so this is where the
/// scan-time audience check gets wired in.
#[derive(Debug)]
pub struct ViewInstanceTableFunction {
    lakehouse: Arc<LakehouseContext>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    guard: Arc<AudienceGuard>,
}

impl ViewInstanceTableFunction {
    pub fn new(
        lakehouse: Arc<LakehouseContext>,
        view_factory: Arc<ViewFactory>,
        part_provider: Arc<dyn QueryPartitionProvider>,
        query_range: Option<TimeRange>,
        guard: Arc<AudienceGuard>,
    ) -> Self {
        Self {
            lakehouse,
            view_factory,
            part_provider,
            query_range,
            guard,
        }
    }
}

impl TableFunctionImpl for ViewInstanceTableFunction {
    #[span_fn]
    fn call_with_args(
        &self,
        args: TableFunctionArgs,
    ) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        let exprs = args.exprs();
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
            Some(self.guard.clone()),
        )))
    }
}
