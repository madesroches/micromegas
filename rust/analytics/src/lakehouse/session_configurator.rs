use anyhow::Result;
use datafusion::execution::context::SessionContext;

/// Trait for configuring a SessionContext with additional tables and settings
///
/// This trait allows users to extend the default session context with custom tables,
/// configuration, or other DataFusion resources. Implementations can register JSON files,
/// CSV files, in-memory tables, or any DataFusion TableProvider.
///
/// # Example
///
/// ```rust,no_run
/// use anyhow::Result;
/// use datafusion::execution::context::SessionContext;
/// use micromegas_analytics::lakehouse::session_configurator::SessionConfigurator;
///
/// #[derive(Debug)]
/// struct MyConfigurator;
///
/// #[async_trait::async_trait]
/// impl SessionConfigurator for MyConfigurator {
///     async fn configure(&self, ctx: &SessionContext) -> Result<()> {
///         // Register custom tables or configure session settings
///         Ok(())
///     }
/// }
/// ```
#[async_trait::async_trait]
pub trait SessionConfigurator: Send + Sync + std::fmt::Debug {
    /// Configure the given SessionContext (e.g., register custom tables)
    ///
    /// # Arguments
    ///
    /// * `ctx` - The SessionContext to configure
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if configuration succeeds, or an error if something goes wrong.
    async fn configure(&self, ctx: &SessionContext) -> Result<()>;
}

/// Default no-op implementation of SessionConfigurator
///
/// This implementation does nothing and is provided as a convenient default
/// when no custom session configuration is needed.
#[derive(Debug, Clone, Default)]
pub struct NoOpSessionConfigurator;

#[async_trait::async_trait]
impl SessionConfigurator for NoOpSessionConfigurator {
    async fn configure(&self, _ctx: &SessionContext) -> Result<()> {
        Ok(())
    }
}
