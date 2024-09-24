use crate::{lakehouse::table_provider::MaterializedView, time::TimeRange};

use super::{answer::Answer, partition_cache::QueryPartitionProvider, view::View};
use anyhow::{Context, Result};
use datafusion::{
    arrow::array::RecordBatch,
    execution::{context::SessionContext, object_store::ObjectStoreUrl},
    sql::TableReference,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

pub async fn query_single_view(
    lake: Arc<DataLakeConnection>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: TimeRange,
    sql: &str,
    view: Arc<dyn View>,
) -> Result<Answer> {
    let view_set_name = view.get_view_set_name().to_string();
    let view_instance_id = view.get_view_instance_id().to_string();
    info!("query {view_set_name} {view_instance_id} sql={sql}");
    view.jit_update(lake.clone(), query_range.begin, query_range.end)
        .await
        .with_context(|| "jit_update")?;
    let ctx = SessionContext::new();
    let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
    let object_store = lake.blob_storage.inner();
    ctx.register_object_store(object_store_url.as_ref(), object_store.clone());
    let table = MaterializedView::new(
        object_store,
        view.clone(),
        part_provider,
        query_range.clone(),
    );
    let full_table_name = format!("__full_{}", &view_set_name);
    ctx.register_table(
        TableReference::Bare {
            table: full_table_name.clone().into(),
        },
        Arc::new(table),
    )?;

    let filtering_table_provider = view
        .make_filtering_table_provider(&ctx, &full_table_name, query_range.begin, query_range.end)
        .await
        .with_context(|| "make_filtering_table_provider")?;

    ctx.register_table(
        TableReference::Bare {
            table: view_set_name.into(),
        },
        filtering_table_provider,
    )?;

    let df = ctx.sql(sql).await?;
    let schema = df.schema().inner().clone();
    let batches: Vec<RecordBatch> = df.collect().await?;
    Ok(Answer::new(schema, batches))
}
