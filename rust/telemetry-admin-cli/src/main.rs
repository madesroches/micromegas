//! Telemetry Admin CLI

// crate-specific lint exceptions:
//#![]

mod lake_size;
mod process_log;
mod process_metrics;
mod process_search;
mod process_thread_events;

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use clap::{Parser, Subcommand};
use lake_size::{delete_old_blocks, fill_block_sizes};
use analytics::alloc_sql_pool;
use lgn_blob_storage::AwsS3BlobStorage;
use lgn_blob_storage::AwsS3Url;
use lgn_blob_storage::BlobStorage;
use lgn_blob_storage::LocalBlobStorage;
use telemetry_sink::TelemetryGuard;
use process_log::{print_logs_by_process, print_process_log};
use process_search::print_process_search;
use process_search::print_process_tree;
use process_search::print_recent_processes;

use crate::{
    process_metrics::print_process_metrics,
    process_thread_events::{print_chrome_trace, print_process_thread_events},
};

#[derive(Parser, Debug)]
#[clap(name = "Legion Telemetry Admin")]
#[clap(about = "CLI to query a local telemetry data lake", version, author)]
#[clap(arg_required_else_help(true))]
struct Cli {
    #[clap(short, long)]
    local: Option<PathBuf>,

    #[clap(short, long, name = "remote-db-url")]
    remote_db_url: Option<String>,

    #[clap(short, long, name = "s3-lake-url")]
    s3_lake_url: Option<String>,

    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Prints a list of recent processes
    #[clap(name = "recent-processes")]
    RecentProcesses,
    /// Prints a list of recent processes matching the provided string
    #[clap(name = "find-processes")]
    FindProcesses {
        /// executable name filter
        filter: String,
    },
    /// Lists the process and its subprocesses
    #[clap(name = "process-tree")]
    ProcessTree {
        /// process guid
        process_id: String,
    },
    /// Prints the logs of recent processes
    #[clap(name = "logs-by-process")]
    LogsByProcess,
    /// Prints the log streams of the process
    #[clap(name = "process-log")]
    ProcessLog {
        /// process guid
        process_id: String,
    },
    /// Prints the thread streams of the process
    #[clap(name = "process-thread-events")]
    ProcessThreadEvents {
        /// process guid
        process_id: String,
    },
    /// Outputs a file compatible with chrome://tracing/
    #[clap(name = "print-chrome-trace")]
    PrintChromeTrace {
        /// process guid
        process_id: String,
    },
    /// Prints the metrics streams of the process
    #[clap(name = "process-metrics")]
    ProcessMetrics {
        /// process guid
        process_id: String,
    },
    #[clap(name = "fill-block-sizes")]
    FillBlockSizes,

    /// Delete blocks x days old or older
    #[clap(name = "delete-old-blocks")]
    DeleteoldBlocks { min_days_old: i32 },
}

#[tokio::main]
async fn main() -> Result<()> {
    let _telemetry_guard = TelemetryGuard::default().unwrap();

    let args = Cli::parse();

    let pool;
    let blob_storage: Arc<dyn BlobStorage>;

    if let Some(local_path) = args.local {
        if args.remote_db_url.is_some() {
            bail!("remote-db-url and local path can't be both specified");
        }
        let blocks_folder = local_path.join("blobs");
        blob_storage = Arc::new(LocalBlobStorage::new(blocks_folder).await?);
        pool = alloc_sql_pool(&local_path).await.unwrap();
    } else {
        if args.remote_db_url.is_none() {
            bail!("remote-db-url or local path has to be specified");
        }

        if args.s3_lake_url.is_none() {
            bail!("s3-lake-url is required when connecting to a remote data lake");
        }

        blob_storage =
            Arc::new(AwsS3BlobStorage::new(AwsS3Url::from_str(&args.s3_lake_url.unwrap())?).await);

        pool = sqlx::any::AnyPoolOptions::new()
            .connect(&args.remote_db_url.unwrap())
            .await
            .with_context(|| String::from("Connecting to telemetry database"))?;
    }

    let mut connection = pool.acquire().await.unwrap();
    match args.command {
        Commands::RecentProcesses => {
            print_recent_processes(&mut connection).await;
        }
        Commands::FindProcesses { filter } => {
            print_process_search(&mut connection, &filter).await;
        }
        Commands::ProcessTree { process_id } => {
            print_process_tree(&pool, &process_id).await?;
        }
        Commands::LogsByProcess => {
            print_logs_by_process(&mut connection, blob_storage).await?;
        }
        Commands::ProcessLog { process_id } => {
            print_process_log(&mut connection, blob_storage, process_id).await?;
        }
        Commands::ProcessThreadEvents { process_id } => {
            print_process_thread_events(&mut connection, blob_storage, &process_id).await?;
        }
        Commands::PrintChromeTrace { process_id } => {
            print_chrome_trace(&pool, blob_storage, &process_id).await?;
        }
        Commands::ProcessMetrics { process_id } => {
            print_process_metrics(&mut connection, blob_storage, &process_id).await?;
        }
        Commands::FillBlockSizes => {
            fill_block_sizes(&mut connection, blob_storage).await?;
        }
        Commands::DeleteoldBlocks { min_days_old } => {
            delete_old_blocks(&mut connection, blob_storage, min_days_old).await?;
        }
    }
    Ok(())
}
