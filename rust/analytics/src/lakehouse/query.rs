use super::{
    answer::Answer, partition_cache::QueryPartitionProvider, property_get_function::PropertyGet,
    view::View, view_factory::ViewFactory,
};
use crate::{
    lakehouse::{
        table_provider::MaterializedView, table_scan_rewrite::TableScanRewrite,
        view_instance_table_function::ViewInstanceTableFunction,
    },
    time::TimeRange,
};
use anyhow::{Context, Result};
use datafusion::{
    arrow::array::RecordBatch,
    execution::{context::SessionContext, object_store::ObjectStoreUrl},
    logical_expr::ScalarUDF,
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
    let table = MaterializedView::new(
        lake,
        object_store,
        view.clone(),
        part_provider,
        query_range.clone(),
    );
    let view_set_name = view.get_view_set_name().to_string();
    ctx.register_table(
        TableReference::Bare {
            table: view_set_name.into(),
        },
        Arc::new(table),
    )?;
    Ok(())
}

pub async fn query_single_view(
    lake: Arc<DataLakeConnection>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    sql: &str,
    view: Arc<dyn View>,
) -> Result<Answer> {
    let view_set_name = view.get_view_set_name().to_string();
    let view_instance_id = view.get_view_instance_id().to_string();
    info!("query_single_view {view_set_name} {view_instance_id} sql={sql}");
    let ctx = SessionContext::new();
    if let Some(range) = &query_range {
        ctx.add_analyzer_rule(Arc::new(TableScanRewrite::new(range.clone())));
    }
    let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
    let object_store = lake.blob_storage.inner();
    ctx.register_object_store(object_store_url.as_ref(), object_store.clone());
    register_table(lake, part_provider, query_range, &ctx, object_store, view).await?;
    let df = ctx.sql(sql).await?;
    let schema = df.schema().inner().clone();
    let batches: Vec<RecordBatch> = df.collect().await?;
    Ok(Answer::new(schema, batches))
}

pub async fn make_session_context(
    lake: Arc<DataLakeConnection>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    view_factory: Arc<ViewFactory>,
) -> Result<SessionContext> {
    let ctx = SessionContext::new();
    if let Some(range) = &query_range {
        ctx.add_analyzer_rule(Arc::new(TableScanRewrite::new(range.clone())));
    }
    let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
    let object_store = lake.blob_storage.inner();
    ctx.register_udtf(
        "view_instance",
        Arc::new(ViewInstanceTableFunction::new(
            lake.clone(),
            object_store.clone(),
            view_factory.clone(),
            part_provider.clone(),
            query_range.clone(),
        )),
    );

    ctx.register_udf(ScalarUDF::from(PropertyGet::new()));

    ctx.register_object_store(object_store_url.as_ref(), object_store.clone());
    for view in view_factory.get_global_views() {
        register_table(
            lake.clone(),
            part_provider.clone(),
            query_range.clone(),
            &ctx,
            object_store.clone(),
            view.clone(),
        )
        .await?;
    }
    Ok(ctx)
}

pub async fn query(
    lake: Arc<DataLakeConnection>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    sql: &str,
    view_factory: Arc<ViewFactory>,
) -> Result<Answer> {
    info!("query sql={sql}");
    let ctx = make_session_context(lake, part_provider, query_range, view_factory)
        .await
        .with_context(|| "make_session_context")?;
    let df = ctx.sql(sql).await?;
    let schema = df.schema().inner().clone();
    let batches: Vec<RecordBatch> = df.collect().await?;
    Ok(Answer::new(schema, batches))
}
