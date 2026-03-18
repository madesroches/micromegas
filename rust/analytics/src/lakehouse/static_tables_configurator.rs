use anyhow::Result;
use datafusion::catalog::TableProvider;
use datafusion::execution::context::SessionContext;
use futures::StreamExt;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

use super::session_configurator::SessionConfigurator;
use crate::dfext::csv_table_provider::csv_table_provider;
use crate::dfext::json_table_provider::json_table_provider;

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

impl StaticTablesConfigurator {
    /// Discovers JSON and CSV files under the given URL and creates table providers for each.
    ///
    /// Files with `.json` extensions are loaded as JSON tables.
    /// Files with `.csv` extensions are loaded as CSV tables.
    /// Other extensions are skipped with a warning.
    ///
    /// Errors loading individual files are logged but do not prevent other files from loading.
    pub async fn new(ctx: &SessionContext, url: &str) -> Result<Self> {
        let parsed_url = url::Url::parse(url)?;
        let (object_store, prefix) = object_store::parse_url_opts(
            &parsed_url,
            std::env::vars().map(|(k, v)| (k.to_lowercase(), v)),
        )?;
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
                            debug!("skipping file without extension: {path_str}");
                            continue;
                        }
                    };

                    if stem.is_empty() {
                        warn!("skipping file with empty stem: {path_str}");
                        continue;
                    }

                    let table_name = stem.to_string();

                    let provider_result = match ext.as_str() {
                        "json" => json_table_provider(ctx, &file_url).await,
                        "csv" => csv_table_provider(ctx, &file_url).await,
                        _ => {
                            warn!("skipping file with unsupported extension: {path_str}");
                            continue;
                        }
                    };

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
