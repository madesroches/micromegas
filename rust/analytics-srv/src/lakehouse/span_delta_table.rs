use anyhow::{Context, Result};
use futures::TryStreamExt;
use lgn_analytics::prelude::*;
use lgn_analytics::time::ConvertTicks;
use lgn_blob_storage::BlobStorage;
use lgn_tracing::prelude::*;
use prost::Message;
use sqlx::Row;
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::{collections::HashMap, path::PathBuf};

use deltalake::{
    action::Protocol, DeltaTable, DeltaTableMetaData, Schema, SchemaDataType, SchemaField,
};

use crate::lakehouse::span_table_partition::write_local_partition;

#[span_fn]
fn get_delta_schema() -> Schema {
    Schema::new(vec![
        SchemaField::new(
            "block_id".to_string(),
            SchemaDataType::primitive("string".to_string()),
            false,
            HashMap::new(),
        ),
        SchemaField::new(
            "thread_id".to_string(),
            SchemaDataType::primitive("string".to_string()),
            false,
            HashMap::new(),
        ),
        SchemaField::new(
            "hash".to_string(),
            SchemaDataType::primitive("integer".to_string()),
            false,
            HashMap::new(),
        ),
        SchemaField::new(
            "depth".to_string(),
            SchemaDataType::primitive("integer".to_string()),
            false,
            HashMap::new(),
        ),
        SchemaField::new(
            "begin_ms".to_string(),
            SchemaDataType::primitive("double".to_string()),
            false,
            HashMap::new(),
        ),
        SchemaField::new(
            "end_ms".to_string(),
            SchemaDataType::primitive("double".to_string()),
            false,
            HashMap::new(),
        ),
        SchemaField::new(
            "id".to_string(),
            SchemaDataType::primitive("long".to_string()),
            false,
            HashMap::new(),
        ),
        SchemaField::new(
            "parent".to_string(),
            SchemaDataType::primitive("long".to_string()),
            false,
            HashMap::new(),
        ),
    ])
}

#[span_fn]
async fn create_empty_delta_table(table_uri: &str) -> Result<DeltaTable> {
    info!("creating table {}", table_uri);
    let storage = deltalake::storage::get_backend_for_uri(table_uri)?;
    let mut table = deltalake::DeltaTable::new(
        table_uri,
        storage,
        deltalake::DeltaTableConfig {
            require_tombstones: false,
            require_files: false,
        },
    )?;
    let table_schema = get_delta_schema();
    let mut commit_info = serde_json::Map::<String, serde_json::Value>::new();
    commit_info.insert(
        "operation".to_string(),
        serde_json::Value::String("CREATE TABLE".to_string()),
    );
    let protocol = Protocol {
        min_reader_version: 1,
        min_writer_version: 1,
    };
    let metadata = DeltaTableMetaData::new(None, None, None, table_schema, vec![], HashMap::new());
    table
        .create(metadata, protocol, Some(commit_info), None)
        .await?;
    Ok(table)
}

#[span_fn]
async fn open_or_create_table(table_uri: &str) -> Result<DeltaTable> {
    match deltalake::open_table(table_uri).await {
        Ok(table) => Ok(table),
        Err(e) => {
            info!(
                "Error opening table {}: {:?}. Will try to create.",
                table_uri, e
            );
            create_empty_delta_table(table_uri).await
        }
    }
}

#[span_fn]
async fn read_block_payload(
    block_id: &str,
    buffer_from_db: Option<Vec<u8>>,
    blob_storage: Arc<dyn BlobStorage>,
) -> Result<lgn_telemetry_proto::telemetry::BlockPayload> {
    let buffer: Vec<u8> = if let Some(buffer) = buffer_from_db {
        buffer
    } else {
        blob_storage
            .read_blob(block_id)
            .await
            .with_context(|| "reading block payload from blob storage")?
    };

    {
        span_scope!("decode");
        let payload = lgn_telemetry_proto::telemetry::BlockPayload::decode(&*buffer)
            .with_context(|| format!("reading payload {}", &block_id))?;
        Ok(payload)
    }
}

#[span_fn]
pub async fn update_spans_delta_table(
    pool: sqlx::any::AnyPool,
    blob_storage: Arc<dyn BlobStorage>,
    process_id: &str,
    convert_ticks: &ConvertTicks,
    spans_table_path: std::path::PathBuf,
) -> Result<()> {
    let storage_uri = format!("{}", spans_table_path.display());
    let mut table = open_or_create_table(&storage_uri).await?;
    let files_already_in_table = table.get_file_set();
    let mut partition_index: u32 = files_already_in_table.len() as u32;

    let mut handles = vec![];

    let (sender, receiver) = channel();

    let mut connection = pool.acquire().await?;
    let streams = find_process_thread_streams(&mut connection, process_id).await?;
    for stream in streams {
        //todo: do not fetch block info for blocks in files_already_in_table
        let mut block_rows = sqlx::query(
            "SELECT blocks.block_id, blocks.stream_id, blocks.begin_time, blocks.begin_ticks, blocks.end_time, blocks.end_ticks, blocks.nb_objects, blocks.payload_size, payloads.payload
             FROM blocks
             LEFT OUTER JOIN payloads ON blocks.block_id = payloads.block_id
             WHERE stream_id = ?
             ORDER BY begin_time;",
        )
            .bind(&stream.stream_id)
            .fetch( &mut connection );
        while let Some(block_row) = block_rows.try_next().await? {
            partition_index += 1;
            let mut partition_starting_span_id = i64::from(partition_index)
                .checked_shl(31)
                .with_context(|| "building partition starting span id")?;
            let convert_ticks = convert_ticks.clone();
            let blob_storage = blob_storage.clone();
            let stream = stream.clone();
            let spans_table_path = spans_table_path.clone();
            let sender = sender.clone();
            let block = lgn_analytics::map_row_block(&block_row)?;
            let payload_buffer = block_row.try_get("payload")?;
            let partition_folder = PathBuf::from(format!("block_id={}", &block.block_id));
            let filename = partition_folder.join("spans.parquet");
            let filename_string = filename
                .to_str()
                .with_context(|| "converting path to string")?
                .to_string();
            if !files_already_in_table.contains(&*filename_string) {
                let parquet_full_path = spans_table_path.join(&filename);
                handles.push(tokio::spawn(async move {
                    let payload =
                        read_block_payload(&block.block_id, payload_buffer, blob_storage.clone())
                            .await?;
                    let opt_action = write_local_partition(
                        &payload,
                        &stream,
                        &block,
                        convert_ticks,
                        &mut partition_starting_span_id,
                        filename_string,
                        &parquet_full_path,
                    )
                    .await
                    .with_context(|| "writing local partition")?;
                    if let Some(action) = opt_action {
                        sender.send(action)?;
                    }
                    Ok(()) as Result<()>
                }));
            }
        }
    }
    drop(sender);
    for h in handles {
        h.await??;
    }

    let actions: Vec<deltalake::action::Action> = receiver.iter().collect();
    if !actions.is_empty() {
        let mut transaction = table.create_transaction(None);
        transaction.add_actions(actions);
        transaction
            .commit(None, None)
            .await
            .with_context(|| "committing transaction")?;
    }
    Ok(())
}
