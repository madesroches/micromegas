use super::{answer::Answer, view::View};
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
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::Row;
use std::sync::Arc;

#[allow(clippy::too_many_arguments)]
pub async fn query(
    lake: Arc<DataLakeConnection>,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    sql: &str,
    view: Arc<dyn View>,
) -> Result<Answer> {
    view.jit_update(lake.clone(), begin, end)
        .await
        .with_context(|| "jit_update")?;
    let ctx = SessionContext::new();
    let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
    ctx.register_object_store(object_store_url.as_ref(), lake.blob_storage.inner());
    let view_set_name = view.get_view_set_name().to_string();
    let view_instance_id = view.get_view_instance_id().to_string();
    let partitions_to_read = sqlx::query(
        "SELECT file_path
         FROM lakehouse_partitions
         WHERE view_set_name = $1
         AND view_instance_id = $2
         AND min_event_time <= $3
         AND max_event_time >= $4
         AND file_schema_hash = $5;",
    )
    .bind(&view_set_name)
    .bind(&view_instance_id)
    .bind(end)
    .bind(begin)
    .bind(view.get_file_schema_hash())
    .fetch_all(&lake.db_pool)
    .await
    .with_context(|| "listing lakehouse partitions")?;

    let mut urls = vec![];
    for row in partitions_to_read {
        let file_path: String = row
            .try_get("file_path")
            .with_context(|| "getting file_path from row")?;
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
