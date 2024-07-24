use std::sync::Arc;

use anyhow::{Context, Result};
use datafusion::{
    arrow::{array::RecordBatch, datatypes::Schema},
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

pub async fn query(
    lake: &DataLakeConnection,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    sql: &str,
    table_set_name: &str,
    table_instance_id: &str,
    latest_schema_hash: &[u8],
    schema: Arc<Schema>,
) -> Result<Vec<RecordBatch>> {
    let ctx = SessionContext::new();
    let object_store_url = ObjectStoreUrl::parse("obj://lakehouse/").unwrap();
    ctx.register_object_store(object_store_url.as_ref(), lake.blob_storage.inner());

    let partitions_to_read = sqlx::query(
        "SELECT file_path
         FROM lakehouse_partitions
         WHERE table_set_name = $1
         AND table_instance_id = $2
         AND min_event_time <= $3
         AND max_event_time >= $4
         AND file_schema_hash = $5;",
    )
    .bind(table_set_name)
    .bind(table_instance_id)
    .bind(end)
    .bind(begin)
    .bind(latest_schema_hash)
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
        .with_schema(schema)
        .with_listing_options(options);
    let table = ListingTable::try_new(config)?;
    ctx.register_table(
        TableReference::Bare {
            table: table_set_name.into(),
        },
        Arc::new(table),
    )?;

    let df = ctx.sql(sql).await?;
    let results: Vec<RecordBatch> = df.collect().await?;
    Ok(results)
}
