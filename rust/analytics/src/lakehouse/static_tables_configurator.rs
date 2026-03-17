use anyhow::Result;
use datafusion::catalog::TableProvider;
use datafusion::datasource::file_format::csv::CsvFormat;
use datafusion::datasource::file_format::json::JsonFormat;
use datafusion::datasource::listing::{
    ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
};
use datafusion::execution::context::SessionContext;
use futures::StreamExt;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

use super::session_configurator::SessionConfigurator;
use crate::dfext::json_table_provider::verify_files_exist;

/// A SessionConfigurator that auto-discovers JSON and CSV files under an object store URL
/// and registers each as a queryable DataFusion table.
///
/// Table names are derived from the filename stem (e.g., `event_schemas.json` → `event_schemas`).
///
/// # Example
///
/// ```rust,no_run
/// use anyhow::Result;
/// use datafusion::execution::context::SessionContext;
/// use micromegas_analytics::lakehouse::static_tables_configurator::StaticTablesConfigurator;
/// use micromegas_analytics::lakehouse::session_configurator::SessionConfigurator;
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     let ctx = SessionContext::new();
///     let configurator = StaticTablesConfigurator::new(&ctx, "file:///data/tables/").await?;
///     // Later, configure a session:
///     configurator.configure(&ctx).await?;
///     Ok(())
/// }
/// ```
#[derive(Debug)]
pub struct StaticTablesConfigurator {
    tables: Vec<(String, Arc<dyn TableProvider>)>,
}

async fn make_listing_table(
    ctx: &SessionContext,
    url: &str,
    listing_options: ListingOptions,
) -> Result<Arc<dyn TableProvider>> {
    let table_url = ListingTableUrl::parse(url)?;
    let object_store = ctx.state().runtime_env().object_store(&table_url)?;
    verify_files_exist(&object_store, table_url.prefix(), url).await?;
    let mut config = ListingTableConfig::new(table_url).with_listing_options(listing_options);
    config = config.infer_schema(&ctx.state()).await?;
    let listing_table = ListingTable::try_new(config)?;
    Ok(Arc::new(listing_table))
}

impl StaticTablesConfigurator {
    /// Discovers JSON and CSV files under the given URL and creates table providers for each.
    ///
    /// Files with `.json` or `.jsonl` extensions are loaded as JSON tables.
    /// Files with `.csv` extensions are loaded as CSV tables.
    /// Other extensions are skipped with a warning.
    ///
    /// Errors loading individual files are logged but do not prevent other files from loading.
    pub async fn new(ctx: &SessionContext, url: &str) -> Result<Self> {
        let parsed_url = url::Url::parse(url)?;
        let (object_store, prefix) = object_store::parse_url(&parsed_url)?;
        let object_store = Arc::new(object_store);

        // Register the object store so table providers can access it
        ctx.register_object_store(&parsed_url, object_store.clone());

        let mut tables = Vec::new();
        let mut list_stream = object_store.list(Some(&prefix));

        while let Some(result) = list_stream.next().await {
            match result {
                Ok(meta) => {
                    let path_str = meta.location.to_string();
                    let file_url = format!(
                        "{scheme}://{authority}/{path}",
                        scheme = parsed_url.scheme(),
                        authority = parsed_url.authority(),
                        path = path_str,
                    );

                    let file_name = meta.location.filename().unwrap_or_default();

                    let (stem, ext) = match file_name.rsplit_once('.') {
                        Some((s, e)) => (s, e.to_lowercase()),
                        None => {
                            warn!("skipping file without extension: {path_str}");
                            continue;
                        }
                    };

                    if stem.is_empty() {
                        warn!("skipping file with empty stem: {path_str}");
                        continue;
                    }

                    let table_name = stem.to_string();

                    let listing_options = match ext.as_str() {
                        "json" | "jsonl" => ListingOptions::new(Arc::new(JsonFormat::default()))
                            .with_file_extension(format!(".{ext}")),
                        "csv" => ListingOptions::new(Arc::new(CsvFormat::default()))
                            .with_file_extension(".csv"),
                        _ => {
                            warn!("skipping file with unsupported extension: {path_str}");
                            continue;
                        }
                    };

                    let provider_result = make_listing_table(ctx, &file_url, listing_options).await;

                    match provider_result {
                        Ok(provider) => {
                            // Check for table name collisions with already-registered tables
                            if ctx.table_provider(&table_name).await.is_ok() {
                                warn!(
                                    "skipping static table '{table_name}': name already registered"
                                );
                                continue;
                            }
                            info!("discovered static table: {table_name} from {path_str}");
                            tables.push((table_name, provider));
                        }
                        Err(e) => {
                            warn!("failed to load static table from {path_str}: {e}");
                        }
                    }
                }
                Err(e) => {
                    warn!("error listing files under {url}: {e}");
                }
            }
        }

        info!(
            "static tables configurator discovered {} tables from {url}",
            tables.len()
        );
        Ok(Self { tables })
    }
}

#[async_trait::async_trait]
impl SessionConfigurator for StaticTablesConfigurator {
    async fn configure(&self, ctx: &SessionContext) -> Result<()> {
        for (name, provider) in &self.tables {
            if let Err(e) = ctx.register_table(name, provider.clone()) {
                warn!("failed to register static table '{name}': {e}");
            }
        }
        Ok(())
    }
}
