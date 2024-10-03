use super::{
    partition_cache::QueryPartitionProvider, table_provider::MaterializedView,
    view_factory::ViewFactory,
};
use crate::time::TimeRange;
use datafusion::{
    catalog::TableProvider, common::plan_err, datasource::function::TableFunctionImpl,
    error::DataFusionError, logical_expr::Expr, scalar::ScalarValue,
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
pub struct ViewInstanceTableFunction {
    lake: Arc<DataLakeConnection>,
    object_store: Arc<dyn ObjectStore>,
    view_factory: Arc<ViewFactory>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
}

impl ViewInstanceTableFunction {
    pub fn new(
        lake: Arc<DataLakeConnection>,
        object_store: Arc<dyn ObjectStore>,
        view_factory: Arc<ViewFactory>,
        part_provider: Arc<dyn QueryPartitionProvider>,
        query_range: Option<TimeRange>,
    ) -> Self {
        Self {
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
        let Some(Expr::Literal(ScalarValue::Utf8(Some(view_set_name)))) = exprs.first() else {
            return plan_err!(
                "First argument to view_instance must be a string (the view set name)"
            );
        };
        let Some(Expr::Literal(ScalarValue::Utf8(Some(view_instance_id)))) = exprs.get(1) else {
            return plan_err!(
                "Second argument to view_instance must be a string (the view instance id)"
            );
        };

        let view = self
            .view_factory
            .make_view(view_set_name, view_instance_id)
            .map_err(|e| DataFusionError::Plan(format!("error making view {e:?}")))?;

        Ok(Arc::new(MaterializedView::new(
            self.lake.clone(),
            self.object_store.clone(),
            view,
            self.part_provider.clone(),
            self.query_range.clone(),
        )))
    }
}
