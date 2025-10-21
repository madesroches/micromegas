use anyhow::Result;
use datafusion::catalog::TableProvider;
use datafusion::datasource::file_format::json::JsonFormat;
use datafusion::datasource::listing::{
    ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
};
use datafusion::execution::context::SessionContext;
use futures::StreamExt;
use object_store::ObjectStore;
use std::sync::Arc;

/// Verifies that files exist at the specified URL
///
/// This function checks if files exist by first attempting to get metadata using
/// `head()`, and if that fails (e.g., for directory patterns), it falls back to
/// listing files at the prefix.
///
/// # Arguments
///
/// * `object_store` - The object store to query
/// * `prefix` - The path/prefix to check for files
/// * `url` - The original URL (used for error messages)
///
/// # Returns
///
/// Returns `Ok(())` if files exist, or an error if no files are found.
async fn verify_files_exist(
    object_store: &Arc<dyn ObjectStore>,
    prefix: &object_store::path::Path,
    url: &str,
) -> Result<()> {
    // Try to get metadata for the file to verify it exists
    let head_result = object_store.head(prefix).await;
    if head_result.is_err() {
        // If head fails, try listing - could be a directory/prefix
        let mut list_stream = object_store.list(Some(prefix));
        let first_file = list_stream.next().await;
        if first_file.is_none() {
            anyhow::bail!("No files found at URL: {}", url);
        }
    }
    Ok(())
}

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
///     let ctx = SessionContext::new();
///     // Create table provider with pre-computed schema (done once at startup)
///     let table = json_table_provider(&ctx, "file:///path/to/data.json").await?;
///
///     // Register in session context (fast, no schema inference)
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
pub async fn json_table_provider(
    ctx: &SessionContext,
    url: &str,
) -> Result<Arc<dyn TableProvider>> {
    let file_format = Arc::new(JsonFormat::default());
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
