use std::sync::Arc;

use micromegas_analytics::prelude::*;
use anyhow::Result;
use lgn_blob_storage::BlobStorage;
use lgn_telemetry_proto::telemetry::Process as ProcessInfo;

pub async fn print_process_log(
    connection: &mut sqlx::PgConnection,
    blob_storage: Arc<dyn BlobStorage>,
    process_id: impl Into<String>,
) -> Result<()> {
    let process = ProcessInfo {
        process_id: process_id.into(),
        ..ProcessInfo::default()
    };

    for_each_process_log_entry(connection, blob_storage, &process, |log_entry| {
        println!(
            "[{}][{}] {}",
            log_entry.level, log_entry.target, log_entry.msg
        );
    })
    .await?;
    Ok(())
}

pub async fn print_logs_by_process(
    connection: &mut sqlx::PgConnection,
    blob_storage: Arc<dyn BlobStorage>,
) -> Result<()> {
    for p in list_recent_processes(connection, None).await.unwrap() {
        let process_info = p.process_info.unwrap();
        println!(
            "{} {} {}",
            process_info.start_time, process_info.process_id, process_info.exe
        );
        print_process_log(connection, blob_storage.clone(), &process_info.process_id).await?;
        println!();
    }
    Ok(())
}
