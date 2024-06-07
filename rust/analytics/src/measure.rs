use crate::{fetch_block_payload, parse_block, time::ConvertTicks};
use anyhow::{Context, Result};
use micromegas_telemetry::{
    blob_storage::BlobStorage, stream_info::StreamInfo, types::block::BlockMetadata,
};
use micromegas_tracing::prelude::*;
use micromegas_transit::Value;
use std::sync::Arc;

pub struct Measure {
    pub time: i64,
    pub target: Arc<String>,
    pub name: Arc<String>,
    pub unit: Arc<String>,
    pub value: f64,
}

pub fn measure_from_value(convert_ticks: &ConvertTicks, val: &Value) -> Result<Option<Measure>> {
    if let Value::Object(obj) = val {
        match obj.type_name.as_str() {
            "FloatMetricEvent" => {
                let ticks = obj
                    .get::<i64>("time")
                    .with_context(|| "reading time from FloatMetricEvent")?;
                let value = obj
                    .get::<f64>("value")
                    .with_context(|| "reading value from FloatMetricEvent")?;
                let desc = obj
                    .get::<Arc<micromegas_transit::Object>>("desc")
                    .with_context(|| "reading desc from FloatMetricEvent")?;
                let target = desc
                    .get::<Arc<String>>("target")
                    .with_context(|| "reading target from FloatMetricEvent")?;
                let name = desc
                    .get::<Arc<String>>("name")
                    .with_context(|| "reading name from FloatMetricEvent")?;
                let unit = desc
                    .get::<Arc<String>>("unit")
                    .with_context(|| "reading unit from FloatMetricEvent")?;
                Ok(Some(Measure {
                    time: convert_ticks.ticks_to_nanoseconds(ticks),
                    target,
                    name,
                    unit,
                    value,
                }))
            }
            "IntegerMetricEvent" => {
                let ticks = obj
                    .get::<i64>("time")
                    .with_context(|| "reading time from IntegerMetricEvent")?;
                let value = obj
                    .get::<u64>("value")
                    .with_context(|| "reading value from IntegerMetricEvent")?;
                let desc = obj
                    .get::<Arc<micromegas_transit::Object>>("desc")
                    .with_context(|| "reading desc from IntegerMetricEvent")?;
                let target = desc
                    .get::<Arc<String>>("target")
                    .with_context(|| "reading target from IntegerMetricEvent")?;
                let name = desc
                    .get::<Arc<String>>("name")
                    .with_context(|| "reading name from IntegerMetricEvent")?;
                let unit = desc
                    .get::<Arc<String>>("unit")
                    .with_context(|| "reading unit from IntegerMetricEvent")?;
                Ok(Some(Measure {
                    time: convert_ticks.ticks_to_nanoseconds(ticks),
                    target,
                    name,
                    unit,
                    value: value as f64,
                }))
            }
            _ => {
                warn!("unknown metric event {:?}", obj);
                Ok(None)
            }
        }
    } else {
        Ok(None)
    }
}

#[span_fn]
pub async fn for_each_measure_in_block<Predicate: FnMut(Measure) -> Result<bool>>(
    blob_storage: Arc<BlobStorage>,
    convert_ticks: &ConvertTicks,
    stream: &StreamInfo,
    block: &BlockMetadata,
    mut fun: Predicate,
) -> Result<bool> {
    let payload = fetch_block_payload(
        blob_storage,
        stream.process_id,
        stream.stream_id,
        block.block_id,
    )
    .await?;
    let continue_iterating = parse_block(stream, &payload, |val| {
        if let Some(measure) =
            measure_from_value(convert_ticks, &val).with_context(|| "measure_from_value")?
        {
            if !fun(measure)? {
                return Ok(false); //do not continue
            }
        }
        Ok(true) //continue
    })
    .with_context(|| "parse_block")?;
    Ok(continue_iterating)
}
