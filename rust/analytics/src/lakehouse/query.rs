use super::{answer::Answer, partition_cache::QueryPartitionProvider, view::View};
use anyhow::{Context, Result};
use datafusion::{
    arrow::array::RecordBatch,
    datasource::{
        file_format::parquet::ParquetFormat,
        listing::{ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl},
    },
    execution::{context::SessionContext, object_store::ObjectStoreUrl},
    sql::TableReference,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::prelude::*;
use sqlx::types::chrono::{DateTime, Utc};
use std::sync::Arc;

pub async fn query_single_view(
    lake: Arc<DataLakeConnection>,
    part_provider: Arc<dyn QueryPartitionProvider>,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    sql: &str,
    view: Arc<dyn View>,
) -> Result<Answer> {
    let view_set_name = view.get_view_set_name().to_string();
    let view_instance_id = view.get_view_instance_id().to_string();
    info!("query {view_set_name} {view_instance_id} sql={sql}");
    view.jit_update(lake.clone(), begin, end)
        .await
        .with_context(|| "jit_update")?;
    let ctx = SessionContext::new();
    let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
    ctx.register_object_store(object_store_url.as_ref(), lake.blob_storage.inner());
    let partitions = part_provider
        .fetch(
            &view_set_name,
            &view_instance_id,
            begin,
            end,
            view.get_file_schema_hash(),
        )
        .await?;
    let mut urls = vec![];
    for part in partitions {
        let file_path = part.file_path;
        urls.push(
            ListingTableUrl::parse(format!("obj://lakehouse/{file_path}"))
                .with_context(|| "parsing obj://filepath as url")?,
        );
    }

    let file_format = ParquetFormat::default().with_enable_pruning(true);
    let options = ListingOptions::new(Arc::new(file_format));
    let config = ListingTableConfig::new_with_multi_paths(urls)
        .with_schema(view.get_file_schema())
        .with_listing_options(options);
    let table = ListingTable::try_new(config)?;
    let full_table_name = format!("__full_{}", &view_set_name);
    ctx.register_table(
        TableReference::Bare {
            table: full_table_name.clone().into(),
        },
        Arc::new(table),
    )?;

    let filtering_table_provider = view
        .make_filtering_table_provider(&ctx, &full_table_name, begin, end)
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
