//! Telemetry Admin CLI

use anyhow::Context;
use anyhow::Result;
use clap::{Parser, Subcommand};
use micromegas::analytics::delete::delete_old_data;
use micromegas::analytics::lakehouse::batch_update::create_or_update_recent_partitions;
use micromegas::analytics::lakehouse::log_view::LogView;
use micromegas::analytics::lakehouse::migration::migrate_lakehouse;
use micromegas::chrono::TimeDelta;
use micromegas::ingestion::data_lake_connection::connect_to_data_lake;
use micromegas::telemetry_sink::TelemetryGuardBuilder;
use micromegas::tracing::levels::LevelFilter;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[clap(name = "Micromegas Telemetry Admin")]
#[clap(about = "CLI to administer a telemetry data lake", version, author)]
#[clap(arg_required_else_help(true))]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Delete blocks, streams and processes x days old or older
    #[clap(name = "delete-old-data")]
    DeleteOldData { min_days_old: i32 },

    #[clap(name = "create-recent-partitions")]
    CreateRecentPartitions {
        partition_delta_seconds: i64,
        nb_partitions: i32,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let _telemetry_guard = TelemetryGuardBuilder::default()
        .with_ctrlc_handling()
        .with_local_sink_max_level(LevelFilter::Debug)
        .build();

    let args = Cli::parse();

    let connection_string = std::env::var("MICROMEGAS_SQL_CONNECTION_STRING")
        .with_context(|| "reading MICROMEGAS_SQL_CONNECTION_STRING")?;
    let object_store_uri = std::env::var("MICROMEGAS_OBJECT_STORE_URI")
        .with_context(|| "reading MICROMEGAS_OBJECT_STORE_URI")?;
    let data_lake = Arc::new(connect_to_data_lake(&connection_string, &object_store_uri).await?);
    migrate_lakehouse(data_lake.db_pool.clone())
        .await
        .with_context(|| "migrate_lakehouse")?;
    match args.command {
        Commands::DeleteOldData { min_days_old } => {
            delete_old_data(&data_lake, min_days_old).await?;
        }
        Commands::CreateRecentPartitions {
            partition_delta_seconds,
            nb_partitions,
        } => {
            let one_minute = TimeDelta::try_seconds(partition_delta_seconds)
                .with_context(|| "making time delta")?;
            create_or_update_recent_partitions(
                data_lake,
                Arc::new(LogView::default()),
                one_minute,
                nb_partitions,
            )
            .await?;
        }
    }
    Ok(())
}
