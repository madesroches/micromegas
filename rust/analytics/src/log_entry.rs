use crate::{
    metadata::{ProcessMetadata, StreamMetadata},
    payload::{fetch_block_payload, parse_block},
    properties::property_set::PropertySet,
    time::ConvertTicks,
};
use anyhow::{Context, Result};
use micromegas_telemetry::{blob_storage::BlobStorage, types::block::BlockMetadata};
use micromegas_tracing::prelude::*;
use micromegas_transit::value::{Object, Value};
use std::sync::Arc;

/// A single log entry.
///
/// String fields borrow the per-block parse arena, so a `LogEntry` is valid only
/// within the `parse_block` callback that produced it; it must be appended to
/// Arrow (which copies the bytes) before the arena is dropped.
#[derive(Debug)]
pub struct LogEntry<'a> {
    pub process: Arc<ProcessMetadata>,
    pub stream_id: Arc<String>,
    pub block_id: Arc<String>,
    pub insert_time: i64,
    pub time: i64,
    pub level: i32,
    pub target: &'a str,
    pub msg: &'a str,
    pub properties: PropertySet<'a>,
}

/// Creates a `LogEntry` from a `Value`.
#[span_fn]
pub fn log_entry_from_value<'a>(
    convert_ticks: &ConvertTicks,
    process: Arc<ProcessMetadata>,
    stream_id: Arc<String>,
    block_id: Arc<String>,
    block_insert_time_ns: i64,
    val: Value<'a>,
) -> Result<Option<LogEntry<'a>>> {
    if let Value::Object(obj) = val {
        match obj.type_name {
            "LogStaticStrEvent" => {
                let ticks = obj
                    .get::<i64>("time")
                    .with_context(|| "reading time from LogStaticStrEvent")?;
                let desc = obj
                    .get::<&Object>("desc")
                    .with_context(|| "reading desc from LogStaticStrEvent")?;
                let level = desc
                    .get::<u32>("level")
                    .with_context(|| "reading level from LogStaticStrEvent")?;
                let target = desc
                    .get::<&str>("target")
                    .with_context(|| "reading target from LogStaticStrEvent")?;
                let msg = desc
                    .get::<&str>("fmt_str")
                    .with_context(|| "reading fmt_str from LogStaticStrEvent")?;
                Ok(Some(LogEntry {
                    process,
                    stream_id,
                    block_id,
                    insert_time: block_insert_time_ns,
                    time: convert_ticks.ticks_to_nanoseconds(ticks),
                    level: level as i32,
                    target,
                    msg,
                    properties: PropertySet::empty(),
                }))
            }
            "LogStringEvent" | "LogStringEventV2" => {
                let ticks = obj
                    .get::<i64>("time")
                    .with_context(|| "reading time from LogStringEvent")?;
                let desc = obj
                    .get::<&Object>("desc")
                    .with_context(|| "reading desc from LogStringEvent")?;
                let level = desc
                    .get::<u32>("level")
                    .with_context(|| "reading level from LogStringEvent")?;
                let target = desc
                    .get::<&str>("target")
                    .with_context(|| "reading target from LogStringEvent")?;
                let msg = obj
                    .get::<&str>("msg")
                    .with_context(|| "reading msg from LogStringEvent")?;
                Ok(Some(LogEntry {
                    process,
                    stream_id,
                    block_id,
                    insert_time: block_insert_time_ns,
                    time: convert_ticks.ticks_to_nanoseconds(ticks),
                    level: level as i32,
                    target,
                    msg,
                    properties: PropertySet::empty(),
                }))
            }
            "LogStaticStrInteropEvent" | "LogStringInteropEventV2" | "LogStringInteropEventV3" => {
                let ticks = obj
                    .get::<i64>("time")
                    .with_context(|| format!("reading time from {}", obj.type_name))?;
                let level = obj
                    .get::<u32>("level")
                    .with_context(|| format!("reading level from {}", obj.type_name))?;
                let target = obj
                    .get::<&str>("target")
                    .with_context(|| format!("reading target from {}", obj.type_name))?;
                let msg = obj
                    .get::<&str>("msg")
                    .with_context(|| format!("reading msg from {}", obj.type_name))?;
                Ok(Some(LogEntry {
                    process,
                    stream_id,
                    block_id,
                    insert_time: block_insert_time_ns,
                    time: convert_ticks.ticks_to_nanoseconds(ticks),
                    level: level as i32,
                    target,
                    msg,
                    properties: PropertySet::empty(),
                }))
            }
            "TaggedLogInteropEvent" => {
                let ticks = obj
                    .get::<i64>("time")
                    .with_context(|| format!("reading time from {}", obj.type_name))?;
                let level = obj
                    .get::<u32>("level")
                    .with_context(|| format!("reading level from {}", obj.type_name))?;
                let target = obj
                    .get::<&str>("target")
                    .with_context(|| format!("reading target from {}", obj.type_name))?;
                let msg = obj
                    .get::<&str>("msg")
                    .with_context(|| format!("reading msg from {}", obj.type_name))?;
                let properties = obj
                    .get::<&Object>("properties")
                    .with_context(|| format!("reading properties from {}", obj.type_name))?;
                let time = convert_ticks.ticks_to_nanoseconds(ticks);
                Ok(Some(LogEntry {
                    process,
                    stream_id,
                    block_id,
                    insert_time: block_insert_time_ns,
                    time,
                    level: level as i32,
                    target,
                    msg,
                    properties: properties.into(),
                }))
            }
            "TaggedLogString" => {
                let ticks = obj
                    .get::<i64>("time")
                    .with_context(|| format!("reading time from {}", obj.type_name))?;
                let msg = obj
                    .get::<&str>("msg")
                    .with_context(|| format!("reading msg from {}", obj.type_name))?;
                let desc = obj
                    .get::<&Object>("desc")
                    .with_context(|| format!("reading desc from {}", obj.type_name))?;
                let mut level = desc
                    .get::<u32>("level")
                    .with_context(|| format!("reading level from {}", obj.type_name))?;
                let mut target = desc
                    .get::<&str>("target")
                    .with_context(|| format!("reading target from {}", obj.type_name))?;
                let properties = obj
                    .get::<&Object>("properties")
                    .with_context(|| format!("reading properties from {}", obj.type_name))?;
                for &(prop_name, prop_value) in properties.members {
                    match (prop_name, prop_value) {
                        ("target", Value::String(value_str)) => {
                            target = value_str;
                        }
                        ("level", Value::String(level_str)) => {
                            level = Level::parse(level_str).with_context(|| "parsing log level")?
                                as u32;
                        }
                        (_, _) => {}
                    }
                }
                Ok(Some(LogEntry {
                    process,
                    stream_id,
                    block_id,
                    insert_time: block_insert_time_ns,
                    time: convert_ticks.ticks_to_nanoseconds(ticks),
                    level: level as i32,
                    target,
                    msg,
                    properties: properties.into(),
                }))
            }

            _ => {
                warn!("unknown log event {:?}", obj);
                Ok(None)
            }
        }
    } else {
        Ok(None)
    }
}

/// Iterates over all log entries in a block.
#[span_fn]
pub async fn for_each_log_entry_in_block<Predicate>(
    blob_storage: Arc<BlobStorage>,
    convert_ticks: &ConvertTicks,
    process: Arc<ProcessMetadata>,
    stream: &StreamMetadata,
    block: &BlockMetadata,
    mut fun: Predicate,
) -> Result<bool>
where
    Predicate: for<'a> FnMut(LogEntry<'a>) -> Result<bool>,
{
    let payload = fetch_block_payload(
        blob_storage,
        stream.process_id,
        stream.stream_id,
        block.block_id,
    )
    .await?;
    let stream_id = Arc::new(stream.stream_id.to_string());
    let block_id = Arc::new(block.block_id.to_string());
    let block_insert_time_ns = block.insert_time.timestamp_nanos_opt().unwrap_or_default();
    let continue_iterating = parse_block(stream, &payload, |val| {
        if let Some(log_entry) = log_entry_from_value(
            convert_ticks,
            process.clone(),
            stream_id.clone(),
            block_id.clone(),
            block_insert_time_ns,
            val,
        )
        .with_context(|| "log_entry_from_value")?
            && !fun(log_entry)?
        {
            return Ok(false); //do not continue
        }
        Ok(true) //continue
    })
    .with_context(|| format!("parse_block {}", block.block_id))?;
    Ok(continue_iterating)
}
