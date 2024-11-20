use crate::{
    payload::{fetch_block_payload, parse_block},
    time::ConvertTicks,
};
use anyhow::{Context, Result};
use micromegas_telemetry::{
    blob_storage::BlobStorage, stream_info::StreamInfo, types::block::BlockMetadata,
};
use micromegas_tracing::prelude::*;
use micromegas_transit::value::{Object, Value};
use std::sync::Arc;

pub struct Measure {
    pub process: Arc<ProcessInfo>,
    pub time: i64,
    pub target: Arc<String>,
    pub name: Arc<String>,
    pub unit: Arc<String>,
    pub value: f64,
}

pub fn measure_from_value(
    process: Arc<ProcessInfo>,
    convert_ticks: &ConvertTicks,
    val: &Value,
) -> Result<Option<Measure>> {
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
                    .get::<Arc<Object>>("desc")
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
                    process,
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
                let time = convert_ticks.ticks_to_nanoseconds(ticks);
                let value = obj
                    .get::<u64>("value")
                    .with_context(|| "reading value from IntegerMetricEvent")?;
                let desc = obj
                    .get::<Arc<Object>>("desc")
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
                if *unit == "ticks" {
                    lazy_static::lazy_static! {
                        static ref SECONDS_METRIC_UNIT: Arc<String> = Arc::new( String::from("seconds"));
                    }
                    Ok(Some(Measure {
                        process,
                        time,
                        target,
                        name,
                        unit: SECONDS_METRIC_UNIT.clone(),
                        value: convert_ticks.delta_ticks_to_ms(value as i64) / 1000.0,
                    }))
                } else {
                    Ok(Some(Measure {
                        process,
                        time,
                        target,
                        name,
                        unit,
                        value: value as f64,
                    }))
                }
            }
            "TaggedIntegerMetricEvent" => {
                let ticks = obj
                    .get::<i64>("time")
                    .with_context(|| "reading time from TaggedIntegerMetricEvent")?;
                let time = convert_ticks.ticks_to_nanoseconds(ticks);
                let value = obj
                    .get::<u64>("value")
                    .with_context(|| "reading value from TaggedIntegerMetricEvent")?;
                let desc = obj
                    .get::<Arc<Object>>("desc")
                    .with_context(|| "reading desc from IntegerMetricEvent")?;
                let mut target = desc
                    .get::<Arc<String>>("target")
                    .with_context(|| "reading target from IntegerMetricEvent")?;
                let mut name = desc
                    .get::<Arc<String>>("name")
                    .with_context(|| "reading name from IntegerMetricEvent")?;
                let mut unit = desc
                    .get::<Arc<String>>("unit")
                    .with_context(|| "reading unit from IntegerMetricEvent")?;
                let properties = obj
                    .get::<Arc<Object>>("properties")
                    .with_context(|| "reading properties from TaggedIntegerMetricEvent")?;
                for (prop_name, prop_value) in &properties.members {
                    match (prop_name.as_str(), prop_value) {
                        ("target", Value::String(value_str)) => {
                            target = value_str.clone();
                        }
                        ("name", Value::String(value_str)) => {
                            name = value_str.clone();
                        }
                        ("unit", Value::String(value_str)) => {
                            unit = value_str.clone();
                        }
                        (&_, _) => {}
                    }
                }

                if *unit == "ticks" {
                    lazy_static::lazy_static! {
                        static ref SECONDS_METRIC_UNIT: Arc<String> = Arc::new( String::from("seconds"));
                    }
                    Ok(Some(Measure {
                        process,
                        time,
                        target,
                        name,
                        unit: SECONDS_METRIC_UNIT.clone(),
                        value: convert_ticks.delta_ticks_to_ms(value as i64) / 1000.0,
                    }))
                } else {
                    Ok(Some(Measure {
                        process,
                        time,
                        target,
                        name,
                        unit,
                        value: value as f64,
                    }))
                }
            }
            "TaggedFloatMetricEvent" => {
                let ticks = obj
                    .get::<i64>("time")
                    .with_context(|| "reading time from TaggedFloatMetricEvent")?;
                let time = convert_ticks.ticks_to_nanoseconds(ticks);
                let value = obj
                    .get::<f64>("value")
                    .with_context(|| "reading value from TaggedFloatMetricEvent")?;
                let desc = obj
                    .get::<Arc<Object>>("desc")
                    .with_context(|| "reading desc from TaggedFloatMetricEvent")?;
                let mut target = desc
                    .get::<Arc<String>>("target")
                    .with_context(|| "reading target from TaggedFloatMetricEvent")?;
                let mut name = desc
                    .get::<Arc<String>>("name")
                    .with_context(|| "reading name from TaggedFloatMetricEvent")?;
                let mut unit = desc
                    .get::<Arc<String>>("unit")
                    .with_context(|| "reading unit from TaggedFloatMetricEvent")?;
                let properties = obj
                    .get::<Arc<Object>>("properties")
                    .with_context(|| "reading properties from TaggedFloatMetricEvent")?;
                for (prop_name, prop_value) in &properties.members {
                    match (prop_name.as_str(), prop_value) {
                        ("target", Value::String(value_str)) => {
                            target = value_str.clone();
                        }
                        ("name", Value::String(value_str)) => {
                            name = value_str.clone();
                        }
                        ("unit", Value::String(value_str)) => {
                            unit = value_str.clone();
                        }
                        (&_, _) => {}
                    }
                }
                Ok(Some(Measure {
                    process,
                    time,
                    target,
                    name,
                    unit,
                    value,
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
    process: Arc<ProcessInfo>,
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
        if let Some(measure) = measure_from_value(process.clone(), convert_ticks, &val)
            .with_context(|| "measure_from_value")?
        {
            if !fun(measure)? {
                return Ok(false); //do not continue
            }
        }
        Ok(true) //continue
    })
    .with_context(|| format!("parse_block {}", block.block_id))?;
    Ok(continue_iterating)
}
