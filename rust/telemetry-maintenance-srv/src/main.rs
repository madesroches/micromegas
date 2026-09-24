//! Telemetry maintenance daemon

#[cfg(not(target_os = "windows"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;
micromegas::declare_jemalloc_conf!();

use anyhow::Result;
use clap::Parser;
use micromegas::analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas::analytics::lakehouse::static_tables_configurator::StaticTablesConfigurator;
use micromegas::analytics::lakehouse::view_definition_store::PgViewDefinitionStore;
use micromegas::analytics::lakehouse::view_factory::default_view_factory;
use micromegas::analytics::lakehouse::view_registry::ViewRegistry;
use micromegas::ingestion::data_lake_connection::WritablePolicy;
use micromegas::micromegas_main;
use micromegas::servers::maintenance::daemon;
use micromegas::servers::shutdown::wait_for_sigterm;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[clap(name = "Micromegas Telemetry Maintenance")]
#[clap(
    about = "Maintenance daemon for a Micromegas telemetry data lake",
    version,
    author
)]
struct Cli {
    /// Delete lake data older than this many days (retention horizon)
    #[clap(long, default_value = "90", env = "MICROMEGAS_RETENTION_DAYS")]
    retention_days: i32,

    #[command(flatten)]
    common: micromegas::config::CommonServerArgs,
}

#[micromegas_main(interop_max_level = "info")]
async fn main() -> Result<()> {
    let args = Cli::parse();

    let lakehouse = LakehouseContext::from_env(WritablePolicy::Require).await?;
    let data_lake = lakehouse.lake().clone();
    let base_view_factory = default_view_factory(
        lakehouse.runtime().clone(),
        data_lake.clone(),
        lakehouse.default_audience(),
    )
    .await?;
    // Resolves the same `MICROMEGAS_STATIC_TABLES_URL` the FlightSQL builder uses, instead of a
    // no-op configurator: a DDL-defined view reading a static table must build the same way in
    // both services.
    let session_configurator = StaticTablesConfigurator::from_env(
        "MICROMEGAS_STATIC_TABLES_URL",
        lakehouse.runtime().clone(),
    )
    .await?;
    let view_registry = Arc::new(ViewRegistry::new(
        Arc::new(base_view_factory),
        Arc::new(PgViewDefinitionStore::new(lakehouse.lake().db_pool.clone())),
        lakehouse.runtime().clone(),
        data_lake,
        session_configurator,
    ));
    let grace = args.common.grace();
    daemon(
        lakehouse,
        view_registry,
        args.retention_days,
        wait_for_sigterm(),
        grace,
    )
    .await
}
