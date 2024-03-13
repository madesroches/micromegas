use anyhow::{Context, Result};
use lgn_blob_storage::BlobStorage;
use micromegas_analytics::prelude::*;
use micromegas_transit::prelude::*;
use std::sync::Arc;

pub async fn print_process_thread_events(
    connection: &mut sqlx::PgConnection,
    blob_storage: Arc<dyn BlobStorage>,
    process_id: &str,
) -> Result<()> {
    for stream in find_process_thread_streams(connection, process_id).await? {
        println!("stream {}", stream.stream_id);
        for block in find_stream_blocks(connection, &stream.stream_id).await? {
            println!("block {}", block.block_id);
            let payload = fetch_block_payload(blob_storage.clone(), block.block_id.clone()).await?;
            parse_block(&stream, &payload, |val| {
                if let Value::Object(obj) = val {
                    let time = obj.get::<u64>("time")?;
                    let scope = obj.get::<Arc<Object>>("thread_span_desc")?;
                    let name = scope.get::<Arc<String>>("name")?;
                    let filename = scope.get::<Arc<String>>("file")?;
                    let line = scope.get::<u32>("line")?;
                    println!("{} {} {} {}:{}", time, obj.type_name, name, filename, line);
                }
                Ok(true) //continue
            })?;
            println!();
        }
        println!();
    }
    Ok(())
}

#[allow(clippy::cast_precision_loss)]
async fn extract_process_thread_events(
    connection: &mut sqlx::PgConnection,
    blob_storage: Arc<dyn BlobStorage>,
    process_info: &micromegas_telemetry_sink::ProcessInfo,
    ts_offset: i64,
    inv_tsc_frequency: f64,
) -> Result<json::Array> {
    let mut events = json::Array::new();
    let process_id = &process_info.process_id;
    for stream in find_process_thread_streams(connection, process_id).await? {
        let system_thread_id = &stream.properties["thread-id"];
        for block in find_stream_blocks(connection, &stream.stream_id).await? {
            let payload = fetch_block_payload(blob_storage.clone(), block.block_id.clone()).await?;
            parse_block(&stream, &payload, |val| {
                if let Value::Object(obj) = val {
                    let phase = match obj.type_name.as_str() {
                        "BeginScopeEvent" => "B",
                        "EndScopeEvent" => "E",
                        _ => panic!("unknown event type {}", obj.type_name),
                    };
                    let tick = obj.get::<i64>("time")?;
                    let time = format!("{}", (tick - ts_offset) as f64 * inv_tsc_frequency);
                    let scope = obj.get::<Arc<Object>>("scope")?;
                    let name = scope.get::<Arc<String>>("name")?;
                    let event = json::object! {
                        name: (*name).clone(),
                        cat: "PERF",
                        ph: phase,
                        pid: process_id.clone(),
                        tid: system_thread_id.clone(),
                        ts: time,

                    };
                    events.push(event);
                }
                Ok(true) //continue
            })?;
        }
    }
    Ok(events)
}

#[allow(clippy::cast_precision_loss)]
pub async fn print_chrome_trace(
    pool: &sqlx::PgPool,
    blob_storage: Arc<dyn BlobStorage>,
    process_id: &str,
) -> Result<()> {
    let mut connection = pool.acquire().await?;
    let root_process_info = find_process(&mut connection, process_id).await?;

    let (tx, rx) = std::sync::mpsc::channel();

    let inv_tsc_frequency = 1000.0 * get_process_tick_length_ms(&root_process_info);
    let root_process_start = root_process_info.start_ticks;
    let mut events = json::Array::new();

    for_each_process_in_tree(
        pool,
        &root_process_info,
        0,
        move |process_info, _rec_level| {
            tx.send(process_info.clone()).unwrap();
        },
    )
    .await
    .with_context(|| "print_chrome_trace")?;

    while let Ok(child_process_info) = rx.recv() {
        assert_eq!(
            root_process_info.tsc_frequency,
            child_process_info.tsc_frequency
        );
        let mut child_events = extract_process_thread_events(
            &mut connection,
            blob_storage.clone(),
            &child_process_info,
            root_process_start,
            inv_tsc_frequency,
        )
        .await?;
        events.append(&mut child_events);
    }

    let trace_document = json::object! {
        traceEvents: events,
        displayTimeUnit: "ms",
    };

    println!("{}", trace_document.dump());
    Ok(())
}
