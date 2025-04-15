use super::{
    answer::Answer, list_partitions_table_function::ListPartitionsTableFunction,
    materialize_partitions_table_function::MaterializePartitionsTableFunction,
    partition::Partition, partition_cache::QueryPartitionProvider,
    partitioned_table_provider::PartitionedTableProvider, property_get_function::PropertyGet,
    retire_partitions_table_function::RetirePartitionsTableFunction, view::View,
    view_factory::ViewFactory,
};
use crate::{
    lakehouse::{
        materialized_view::MaterializedView, table_scan_rewrite::TableScanRewrite,
        view_instance_table_function::ViewInstanceTableFunction,
    },
    time::TimeRange,
};
use anyhow::{Context, Result};
use datafusion::{
    arrow::{array::RecordBatch, datatypes::SchemaRef},
    execution::{context::SessionContext, object_store::ObjectStoreUrl, runtime_env::RuntimeEnv},
    logical_expr::ScalarUDF,
    prelude::*,
    sql::TableReference,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use object_store::ObjectStore;
use std::sync::Arc;

async fn register_table(
    lake: Arc<DataLakeConnection>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    ctx: &SessionContext,
    object_store: Arc<dyn ObjectStore>,
    view: Arc<dyn View>,
) -> Result<()> {
    let table = MaterializedView::new(lake, object_store, view.clone(), part_provider, query_range);
    view.register_table(ctx, table).await
}

/// query_partitions returns a dataframe, leaving the option of streaming the results
pub async fn query_partitions(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    schema: SchemaRef,
    partitions: Arc<Vec<Partition>>,
    sql: &str,
) -> Result<DataFrame> {
    let object_store = lake.blob_storage.inner();
    let table = PartitionedTableProvider::new(schema, object_store.clone(), partitions);
    let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
    let ctx = SessionContext::new_with_config_rt(SessionConfig::default(), runtime);
    ctx.register_object_store(object_store_url.as_ref(), object_store.clone());
    ctx.register_table(
        TableReference::Bare {
            table: "source".into(),
        },
        Arc::new(table),
    )?;
    Ok(ctx.sql(sql).await?)
}

pub fn register_functions(
    ctx: &SessionContext,
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    view_factory: Arc<ViewFactory>,
    object_store: Arc<dyn ObjectStore>,
) {
    ctx.register_udtf(
        "view_instance",
        Arc::new(ViewInstanceTableFunction::new(
            lake.clone(),
            object_store,
            view_factory.clone(),
            part_provider.clone(),
            query_range,
        )),
    );
    ctx.register_udtf(
        "list_partitions",
        Arc::new(ListPartitionsTableFunction::new(lake.clone())),
    );
    ctx.register_udtf(
        "retire_partitions",
        Arc::new(RetirePartitionsTableFunction::new(lake.clone())),
    );
    ctx.register_udtf(
        "materialize_partitions",
        Arc::new(MaterializePartitionsTableFunction::new(
            runtime,
            lake.clone(),
            view_factory.clone(),
        )),
    );
    ctx.register_udf(ScalarUDF::from(PropertyGet::new()));
}

pub async fn make_session_context(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    view_factory: Arc<ViewFactory>,
) -> Result<SessionContext> {
    let ctx = SessionContext::new_with_config_rt(SessionConfig::default(), runtime.clone());
    if let Some(range) = &query_range {
        ctx.add_analyzer_rule(Arc::new(TableScanRewrite::new(*range)));
    }
    let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
    let object_store = lake.blob_storage.inner();
    ctx.register_object_store(object_store_url.as_ref(), object_store.clone());
    register_functions(
        &ctx,
        runtime,
        lake.clone(),
        part_provider.clone(),
        query_range,
        view_factory.clone(),
        object_store.clone(),
    );
    for view in view_factory.get_global_views() {
        register_table(
            lake.clone(),
            part_provider.clone(),
            query_range,
            &ctx,
            object_store.clone(),
            view.clone(),
        )
        .await?;
    }
    Ok(ctx)
}

pub async fn query(
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    sql: &str,
    view_factory: Arc<ViewFactory>,
) -> Result<Answer> {
    info!("query sql={sql}");
    let ctx = make_session_context(runtime, lake, part_provider, query_range, view_factory)
        .await
        .with_context(|| "make_session_context")?;
    let df = ctx.sql(sql).await?;
    let schema = df.schema().inner().clone();
    let batches: Vec<RecordBatch> = df.collect().await?;
    Ok(Answer::new(schema, batches))
}
