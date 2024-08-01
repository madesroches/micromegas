//! Telemetry Admin CLI

use anyhow::Context;
use anyhow::Result;
use clap::{Parser, Subcommand};
use micromegas::analytics::delete::delete_old_data;
use micromegas::analytics::lakehouse::batch_update::create_or_update_partitions;
use micromegas::analytics::lakehouse::batch_update::create_or_update_recent_partitions;
use micromegas::analytics::lakehouse::merge::merge_partitions;
use micromegas::analytics::lakehouse::merge::merge_recent_partitions;
use micromegas::analytics::lakehouse::migration::migrate_lakehouse;
use micromegas::analytics::lakehouse::partition::retire_partitions;
use micromegas::analytics::lakehouse::temp::delete_expired_temporary_files;
use micromegas::analytics::lakehouse::view_factory::ViewFactory;
use micromegas::chrono::DateTime;
use micromegas::chrono::TimeDelta;
use micromegas::chrono::Utc;
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

    #[clap(name = "delete-expired-temp")]
    DeleteExpiredTemp,

    #[clap(name = "create-recent-partitions")]
    CreateRecentPartitions {
        view_set_name: String,
        view_instance_id: String,
        partition_delta_seconds: i64,
        nb_partitions: i32,
    },

    #[clap(name = "create-partitions")]
    CreatePartitions {
        view_set_name: String,
        view_instance_id: String,
        begin: DateTime<Utc>,
        end: DateTime<Utc>,
        partition_delta_seconds: i64,
    },

    #[clap(name = "merge-recent-partitions")]
    MergeRecentPartitions {
        view_set_name: String,
        view_instance_id: String,
        partition_delta_seconds: i64,
        nb_partitions: i32,
        min_age_seconds: i64, // do not merge partitions younger than min_age_seconds
    },

    #[clap(name = "merge-partitions")]
    MergePartitions {
        view_set_name: String,
        view_instance_id: String,
        begin: DateTime<Utc>,
        end: DateTime<Utc>,
        partition_delta_seconds: i64,
    },

    #[clap(name = "retire-partitions")]
    RetirePartitions {
        view_set_name: String,
        view_instance_id: String,
        begin: DateTime<Utc>,
        end: DateTime<Utc>,
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
    let view_factory = ViewFactory::default();
    match args.command {
        Commands::DeleteOldData { min_days_old } => {
            delete_old_data(&data_lake, min_days_old).await?;
        }
        Commands::DeleteExpiredTemp => {
            delete_expired_temporary_files(data_lake).await?;
        }
        Commands::CreateRecentPartitions {
            view_set_name,
            view_instance_id,
            partition_delta_seconds,
            nb_partitions,
        } => {
            let delta = TimeDelta::try_seconds(partition_delta_seconds)
                .with_context(|| "making time delta")?;
            create_or_update_recent_partitions(
                data_lake,
                view_factory.make_view(&view_set_name, &view_instance_id)?,
                delta,
                nb_partitions,
            )
            .await?;
        }
        Commands::CreatePartitions {
            view_set_name,
            view_instance_id,
            begin,
            end,
            partition_delta_seconds,
        } => {
            let delta = TimeDelta::try_seconds(partition_delta_seconds)
                .with_context(|| "making time delta")?;
            create_or_update_partitions(
                data_lake,
                view_factory.make_view(&view_set_name, &view_instance_id)?,
                begin,
                end,
                delta,
            )
            .await?;
        }
        Commands::MergeRecentPartitions {
            view_set_name,
            view_instance_id,
            partition_delta_seconds,
            nb_partitions,
            min_age_seconds,
        } => {
            let delta = TimeDelta::try_seconds(partition_delta_seconds)
                .with_context(|| "making time delta")?;
            let min_age =
                TimeDelta::try_seconds(min_age_seconds).with_context(|| "making min_age")?;
            merge_recent_partitions(
                data_lake,
                view_factory.make_view(&view_set_name, &view_instance_id)?,
                delta,
                nb_partitions,
                min_age,
            )
            .await?;
        }
        Commands::MergePartitions {
            view_set_name,
            view_instance_id,
            begin,
            end,
            partition_delta_seconds,
        } => {
            let delta = TimeDelta::try_seconds(partition_delta_seconds)
                .with_context(|| "making time delta")?;
            merge_partitions(
                data_lake,
                view_factory.make_view(&view_set_name, &view_instance_id)?,
                begin,
                end,
                delta,
            )
            .await?;
        }
        Commands::RetirePartitions {
            view_set_name,
            view_instance_id,
            begin,
            end,
        } => {
            let mut tr = data_lake.db_pool.begin().await?;
            retire_partitions(&mut tr, &view_set_name, &view_instance_id, begin, end).await?;
            tr.commit().await.with_context(|| "commit")?;
        }
    }
    Ok(())
}
