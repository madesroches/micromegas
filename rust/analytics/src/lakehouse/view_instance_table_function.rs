use super::{
    materialized_view::MaterializedView, partition_cache::QueryPartitionProvider,
    view_factory::ViewFactory,
};
use crate::{dfext::expressions::exp_to_string, time::TimeRange};
use datafusion::{
    catalog::{TableFunctionImpl, TableProvider},
    common::plan_err,
    error::DataFusionError,
    execution::runtime_env::RuntimeEnv,
    logical_expr::Expr,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use object_store::ObjectStore;
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
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    object_store: Arc<dyn ObjectStore>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
}

impl ViewInstanceTableFunction {
    pub fn new(
        runtime: Arc<RuntimeEnv>,
        lake: Arc<DataLakeConnection>,
        object_store: Arc<dyn ObjectStore>,
        view_factory: Arc<ViewFactory>,
        part_provider: Arc<dyn QueryPartitionProvider>,
        query_range: Option<TimeRange>,
    ) -> Self {
        Self {
            runtime,
            lake,
            object_store,
            view_factory,
            part_provider,
            query_range,
        }
    }
}

impl TableFunctionImpl for ViewInstanceTableFunction {
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
            self.runtime.clone(),
            self.lake.clone(),
            self.object_store.clone(),
            view,
            self.part_provider.clone(),
            self.query_range,
        )))
    }
}
