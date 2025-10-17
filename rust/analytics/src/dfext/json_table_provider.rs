use anyhow::Result;
use datafusion::catalog::TableProvider;
use datafusion::datasource::file_format::json::JsonFormat;
use datafusion::datasource::listing::{
    ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
};
use datafusion::execution::context::SessionContext;
use std::sync::Arc;

/// Creates a TableProvider for a JSON file with pre-computed schema
///
/// This function infers the schema once and returns a TableProvider that can be
/// registered in multiple SessionContexts without re-inferring the schema.
///
/// DataFusion supports **JSONL (newline-delimited JSON)** format, where each line
/// contains a complete JSON object.
///
/// # Arguments
///
/// * `url` - URL to the JSON file (e.g., "file:///path/to/data.json" or "s3://bucket/data.json")
///
/// # Returns
///
/// Returns an `Arc<dyn TableProvider>` that can be registered using
/// `SessionContext::register_table()`.
///
/// # Example
///
/// ```rust,no_run
/// use anyhow::Result;
/// use datafusion::execution::context::SessionContext;
/// use micromegas_analytics::dfext::json_table_provider::json_table_provider;
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     // Create table provider with pre-computed schema (done once at startup)
///     let table = json_table_provider("file:///path/to/data.json").await?;
///
///     // Register in session context (fast, no schema inference)
///     let ctx = SessionContext::new();
///     ctx.register_table("my_table", table)?;
///
///     Ok(())
/// }
/// ```
///
/// # Performance
///
/// Schema inference happens once during this function call. The returned
/// TableProvider caches the schema, making subsequent registrations in
/// different SessionContexts very fast.
pub async fn json_table_provider(url: &str) -> Result<Arc<dyn TableProvider>> {
    let ctx = SessionContext::new();
    let file_format = Arc::new(JsonFormat::default());
    let listing_options = ListingOptions::new(file_format);
    let table_url = ListingTableUrl::parse(url)?;
    let mut config = ListingTableConfig::new(table_url).with_listing_options(listing_options);
    config = config.infer_schema(&ctx.state()).await?;
    let listing_table = ListingTable::try_new(config)?;
    Ok(Arc::new(listing_table))
}
