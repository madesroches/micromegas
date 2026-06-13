//! Telemetry Admin CLI

#[cfg(not(target_os = "windows"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use anyhow::Context;
use anyhow::Result;
use clap::{Parser, Subcommand};
use micromegas::analytics::delete::delete_old_data;
use micromegas::analytics::lakehouse::batch_update::materialize_partition_range;
use micromegas::analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas::analytics::lakehouse::partition_cache::PartitionCache;
use micromegas::analytics::lakehouse::temp::delete_expired_temporary_files;
use micromegas::analytics::lakehouse::view_factory::default_view_factory;
use micromegas::analytics::lakehouse::write_partition::retire_partitions;
use micromegas::analytics::response_writer::ResponseWriter;
use micromegas::analytics::time::TimeRange;
use micromegas::chrono::DateTime;
use micromegas::chrono::TimeDelta;
use micromegas::chrono::Utc;
use micromegas::micromegas_main;
use micromegas::servers::maintenance::get_global_views_with_update_group;
use micromegas::servers::shutdown::wait_for_sigterm;
use std::sync::Arc;
use std::time::Duration;

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

    #[clap(name = "materialize-partitions")]
    MaterializePartitions {
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

    #[clap(name = "crond")]
    CronDaemon {
        /// Seconds to wait for in-flight tasks to complete after SIGTERM
        #[clap(long, default_value = "25")]
        shutdown_grace_period_seconds: u64,
    },
}

#[micromegas_main(interop_max_level = "info")]
async fn main() -> Result<()> {
    let args = Cli::parse();

    let lakehouse = LakehouseContext::from_env().await?;
    let data_lake = lakehouse.lake().clone();
    let view_factory = default_view_factory(lakehouse.runtime().clone(), data_lake.clone()).await?;
    let null_response_writer = Arc::new(ResponseWriter::new(None));
    match args.command {
        Commands::DeleteOldData { min_days_old } => {
            delete_old_data(&data_lake, min_days_old).await?;
        }
        Commands::DeleteExpiredTemp => {
            delete_expired_temporary_files(data_lake).await?;
        }
        Commands::MaterializePartitions {
            view_set_name,
            view_instance_id,
            begin,
            end,
            partition_delta_seconds,
        } => {
            let delta = TimeDelta::try_seconds(partition_delta_seconds)
                .with_context(|| "making time delta")?;
            let insert_range = TimeRange::new(begin, end);
            let existing_partitions_all_views = Arc::new(
                PartitionCache::fetch_overlapping_insert_range(
                    &lakehouse.lake().db_pool,
                    insert_range,
                )
                .await?,
            );
            materialize_partition_range(
                existing_partitions_all_views,
                lakehouse,
                view_factory.make_view(&view_set_name, &view_instance_id)?,
                insert_range,
                delta,
                null_response_writer,
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
            retire_partitions(
                &mut tr,
                &view_set_name,
                &view_instance_id,
                begin,
                end,
                null_response_writer,
            )
            .await?;
            tr.commit().await.with_context(|| "commit")?;
        }

        Commands::CronDaemon {
            shutdown_grace_period_seconds,
        } => {
            let views_to_update = get_global_views_with_update_group(&view_factory);
            let grace = Duration::from_secs(shutdown_grace_period_seconds);
            micromegas::servers::maintenance::daemon(
                lakehouse,
                views_to_update,
                wait_for_sigterm(),
                grace,
            )
            .await?
        }
    }
    Ok(())
}
