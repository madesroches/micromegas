//! Telemetry Admin CLI

// crate-specific lint exceptions:
//#![]

mod lake_size;

use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use clap::{Parser, Subcommand};
use lake_size::delete_old_blocks;
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_telemetry_sink::TelemetryGuard;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[clap(name = "Legion Telemetry Admin")]
#[clap(about = "CLI to query a local telemetry data lake", version, author)]
#[clap(arg_required_else_help(true))]
struct Cli {
    #[clap(short, long, name = "remote-db-url")]
    remote_db_url: Option<String>,

    #[clap(short, long, name = "s3-lake-url")]
    s3_lake_url: Option<String>,

    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Delete blocks x days old or older
    #[clap(name = "delete-old-blocks")]
    DeleteoldBlocks { min_days_old: i32 },
}

#[tokio::main]
async fn main() -> Result<()> {
    let _telemetry_guard = TelemetryGuard::new().unwrap();

    let args = Cli::parse();

    if args.remote_db_url.is_none() {
        bail!("remote-db-url or local path has to be specified");
    }

    if args.s3_lake_url.is_none() {
        bail!("s3-lake-url is required when connecting to a remote data lake");
    }

    let blob_storage = Arc::new(BlobStorage::connect(&args.s3_lake_url.unwrap())?);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect(&args.remote_db_url.unwrap())
        .await
        .with_context(|| String::from("Connecting to telemetry database"))?;
    let mut connection = pool.acquire().await.unwrap();
    match args.command {
        Commands::DeleteoldBlocks { min_days_old } => {
            delete_old_blocks(&mut connection, blob_storage, min_days_old).await?;
        }
    }
    Ok(())
}
