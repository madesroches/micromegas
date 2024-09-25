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
    view.jit_update(lake.clone(), query_range.clone())
        .await
        .with_context(|| "jit_update")?;
    let table = MaterializedView::new(
        object_store,
        view.clone(),
        part_provider,
        query_range.clone(),
    );
    let view_set_name = view.get_view_set_name().to_string();
    if let Some(range) = &query_range {
        let full_table_name = format!("__full_{}", &view_set_name);
        ctx.register_table(
            TableReference::Bare {
                table: full_table_name.clone().into(),
            },
            Arc::new(table),
        )?;

        let filtering_table_provider = view
            .make_filtering_table_provider(ctx, &full_table_name, range.begin, range.end)
            .await
            .with_context(|| "make_filtering_table_provider")?;

        ctx.register_table(
            TableReference::Bare {
                table: view_set_name.into(),
            },
            filtering_table_provider,
        )?;
    } else {
        ctx.register_table(
            TableReference::Bare {
                table: view_set_name.into(),
            },
            Arc::new(table),
        )?;
    }
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
    let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
    let object_store = lake.blob_storage.inner();
    ctx.register_object_store(object_store_url.as_ref(), object_store.clone());
    register_table(lake, part_provider, query_range, &ctx, object_store, view).await?;
    let df = ctx.sql(sql).await?;
    let schema = df.schema().inner().clone();
    let batches: Vec<RecordBatch> = df.collect().await?;
    Ok(Answer::new(schema, batches))
}

pub async fn query(
    lake: Arc<DataLakeConnection>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    query_range: Option<TimeRange>,
    sql: &str,
    views: &[Arc<dyn View>],
) -> Result<Answer> {
    info!("query sql={sql}");
    let ctx = SessionContext::new();
    let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
    let object_store = lake.blob_storage.inner();
    ctx.register_object_store(object_store_url.as_ref(), object_store.clone());
    for view in views {
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
    let df = ctx.sql(sql).await?;
    let schema = df.schema().inner().clone();
    let batches: Vec<RecordBatch> = df.collect().await?;
    Ok(Answer::new(schema, batches))
}
