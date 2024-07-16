use crate::{
    payload::{fetch_block_payload, parse_block},
    time::ConvertTicks,
};
use anyhow::{Context, Result};
use micromegas_telemetry::{
    blob_storage::BlobStorage, stream_info::StreamInfo, types::block::BlockMetadata,
};
use micromegas_tracing::prelude::*;
use micromegas_transit::Value;
use std::sync::Arc;

pub struct LogEntry {
    pub time: i64,
    pub level: i32,
    pub target: Arc<String>,
    pub msg: Arc<String>,
}

#[span_fn]
pub fn log_entry_from_value(convert_ticks: &ConvertTicks, val: &Value) -> Result<Option<LogEntry>> {
    if let Value::Object(obj) = val {
        match obj.type_name.as_str() {
            "LogStaticStrEvent" => {
                let ticks = obj
                    .get::<i64>("time")
                    .with_context(|| "reading time from LogStaticStrEvent")?;
                let desc = obj
                    .get::<Arc<micromegas_transit::Object>>("desc")
                    .with_context(|| "reading desc from LogStaticStrEvent")?;
                let level = desc
                    .get::<u32>("level")
                    .with_context(|| "reading level from LogStaticStrEvent")?;
                let target = desc
                    .get::<Arc<String>>("target")
                    .with_context(|| "reading target from LogStaticStrEvent")?;
                let msg = desc
                    .get::<Arc<String>>("fmt_str")
                    .with_context(|| "reading fmt_str from LogStaticStrEvent")?;
                Ok(Some(LogEntry {
                    time: convert_ticks.ticks_to_nanoseconds(ticks),
                    level: level as i32,
                    target,
                    msg,
                }))
            }
            "LogStringEvent" => {
                let ticks = obj
                    .get::<i64>("time")
                    .with_context(|| "reading time from LogStringEvent")?;
                let desc = obj
                    .get::<Arc<micromegas_transit::Object>>("desc")
                    .with_context(|| "reading desc from LogStringEvent")?;
                let level = desc
                    .get::<u32>("level")
                    .with_context(|| "reading level from LogStringEvent")?;
                let target = desc
                    .get::<Arc<String>>("target")
                    .with_context(|| "reading target from LogStringEvent")?;
                let msg = obj
                    .get::<Arc<String>>("msg")
                    .with_context(|| "reading msg from LogStringEvent")?;
                Ok(Some(LogEntry {
                    time: convert_ticks.ticks_to_nanoseconds(ticks),
                    level: level as i32,
                    target,
                    msg,
                }))
            }
            "LogStaticStrInteropEvent" | "LogStringInteropEventV2" | "LogStringInteropEventV3" => {
                let ticks = obj
                    .get::<i64>("time")
                    .with_context(|| format!("reading time from {}", obj.type_name.as_str()))?;
                let level = obj
                    .get::<u32>("level")
                    .with_context(|| format!("reading level from {}", obj.type_name.as_str()))?;
                let target = obj
                    .get::<Arc<String>>("target")
                    .with_context(|| format!("reading target from {}", obj.type_name.as_str()))?;
                let msg = obj
                    .get::<Arc<String>>("msg")
                    .with_context(|| format!("reading msg from {}", obj.type_name.as_str()))?;
                Ok(Some(LogEntry {
                    time: convert_ticks.ticks_to_nanoseconds(ticks),
                    level: level as i32,
                    target,
                    msg,
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

#[span_fn]
pub async fn for_each_log_entry_in_block<Predicate: FnMut(LogEntry) -> Result<bool>>(
    blob_storage: Arc<BlobStorage>,
    convert_ticks: &ConvertTicks,
    stream: &StreamInfo,
    block: &BlockMetadata,
    mut fun: Predicate,
) -> Result<()> {
    let payload = fetch_block_payload(
        blob_storage,
        stream.process_id,
        stream.stream_id,
        block.block_id,
    )
    .await?;
    parse_block(stream, &payload, |val| {
        if let Some(log_entry) =
            log_entry_from_value(convert_ticks, &val).with_context(|| "log_entry_from_value")?
        {
            if !fun(log_entry)? {
                return Ok(false); //do not continue
            }
        }
        Ok(true) //continue
    })
    .with_context(|| "parse_block")?;
    Ok(())
}
