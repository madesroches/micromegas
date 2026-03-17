use anyhow::Result;
use datafusion::catalog::TableProvider;
use datafusion::datasource::file_format::csv::CsvFormat;
use datafusion::datasource::listing::{
    ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
};
use datafusion::execution::context::SessionContext;
use std::sync::Arc;

use super::json_table_provider::verify_files_exist;

/// Creates a TableProvider for a CSV file with pre-computed schema
///
/// This function infers the schema once and returns a TableProvider that can be
/// registered in multiple SessionContexts without re-inferring the schema.
///
/// Assumes CSV files have a header row and use comma delimiters.
///
/// # Arguments
///
/// * `ctx` - A SessionContext used for schema inference and object store access
/// * `url` - URL to the CSV file (e.g., "file:///path/to/data.csv" or "s3://bucket/data.csv")
///
/// # Returns
///
/// Returns an `Arc<dyn TableProvider>` that can be registered using
/// `SessionContext::register_table()`.
pub async fn csv_table_provider(ctx: &SessionContext, url: &str) -> Result<Arc<dyn TableProvider>> {
    let file_format = Arc::new(CsvFormat::default());
    let listing_options = ListingOptions::new(file_format);
    let table_url = ListingTableUrl::parse(url)?;

    // Verify that files exist at the specified URL
    let object_store = ctx.state().runtime_env().object_store(&table_url)?;
    verify_files_exist(&object_store, table_url.prefix(), url).await?;

    let mut config = ListingTableConfig::new(table_url).with_listing_options(listing_options);
    config = config.infer_schema(&ctx.state()).await?;
    let listing_table = ListingTable::try_new(config)?;
    Ok(Arc::new(listing_table))
}
