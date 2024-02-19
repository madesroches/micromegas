use anyhow::{Context, Ok, Result};
use analytics::find_stream;
use analytics::prelude::*;
use lgn_blob_storage::BlobStorage;
use lgn_telemetry_proto::analytics::MetricBlockData;
use lgn_telemetry_proto::analytics::MetricBlockDesc;
use lgn_telemetry_proto::analytics::MetricBlockManifest;
use lgn_telemetry_proto::analytics::MetricBlockRequest;
use lgn_telemetry_proto::analytics::MetricDataPoint;
use lgn_telemetry_proto::analytics::MetricDesc;
use tracing::prelude::*;
use transit::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use xxhash_rust::const_xxh32::xxh32 as const_xxh32;

use crate::cache::DiskCache;

#[allow(clippy::cast_precision_loss)]
#[allow(dead_code)]
pub async fn get_process_metrics_time_range(
    sql: &mut sqlx::AnyConnection,
    process_id: &str,
) -> Result<(f64, f64)> {
    let mut min_ticks = i64::MAX;
    let mut max_ticks = i64::MIN;
    let process = find_process(sql, process_id).await?;
    for stream in find_process_metrics_streams(sql, process_id).await? {
        for block in find_stream_blocks(sql, &stream.stream_id).await? {
            let block_begin = block.begin_ticks - process.start_ticks;
            let block_end = block.end_ticks - process.start_ticks;
            min_ticks = std::cmp::min(min_ticks, block_begin);
            max_ticks = std::cmp::max(max_ticks, block_end);
        }
    }
    let inv_tsc_frequency = get_process_tick_length_ms(&process);
    Ok((
        min_ticks as f64 * inv_tsc_frequency,
        max_ticks as f64 * inv_tsc_frequency,
    ))
}

#[allow(clippy::cast_possible_wrap)]
fn reduce_lod(source: &MetricBlockData, lod: u32) -> Vec<MetricDataPoint> {
    let merge_threshold = 100.0_f64.powi(lod as i32 - 2) / 10.0;
    let mut points: Vec<MetricDataPoint> = vec![];
    let mut max = f64::MIN;
    let mut acc = 0.0;
    if source.points.is_empty() {
        return points;
    }
    for i in 0..source.points.len() - 1 {
        let point = &source.points[i];
        max = f64::max(point.value, max);
        let next_point = &source.points[i + 1];
        let delta = next_point.time_ms - point.time_ms;
        acc += delta;
        if acc > merge_threshold {
            points.push(MetricDataPoint {
                time_ms: point.time_ms,
                value: max,
            });
            max = f64::MIN;
            acc = 0.0;
        }
    }
    points
}

fn get_lod_block_key(block_id: &str, metric_name: &str, lod: u32) -> String {
    [
        &const_xxh32(metric_name.as_bytes(), 0).to_string(),
        block_id,
        &lod.to_string(),
    ]
    .join("_")
}

fn get_lod_block_request_key(request: &MetricBlockRequest) -> String {
    get_lod_block_key(&request.block_id, &request.metric_name, request.lod)
}

pub struct MetricHandler {
    blob_storage: Arc<dyn BlobStorage>,
    pool: sqlx::any::AnyPool,
    cache: Arc<DiskCache>,
}

impl MetricHandler {
    pub fn new(
        blob_storage: Arc<dyn BlobStorage>,
        cache: Arc<DiskCache>,
        pool: sqlx::any::AnyPool,
    ) -> Self {
        Self {
            blob_storage,
            pool,
            cache,
        }
    }

    pub async fn get_block_lod_data(&self, request: MetricBlockRequest) -> Result<MetricBlockData> {
        Ok(self
            .cache
            .get_or_put(&get_lod_block_request_key(&request), async {
                let raw = self
                    .cache
                    .get_or_put(
                        &get_lod_block_key(&request.block_id, &request.metric_name, 0),
                        async {
                            Ok(self
                                .get_raw_block_data(
                                    &request.process_id,
                                    &request.block_id,
                                    &request.stream_id,
                                    &request.metric_name,
                                )
                                .await?)
                        },
                    )
                    .await?;
                if request.lod > 0 {
                    Ok(MetricBlockData {
                        lod: request.lod,
                        points: reduce_lod(&raw, request.lod),
                    })
                } else {
                    Ok(raw)
                }
            })
            .await?)
    }

    #[allow(clippy::cast_precision_loss)]
    pub async fn get_block_manifest(
        &self,
        process_id: &str,
        block_id: &str,
        stream_id: &str,
    ) -> Result<MetricBlockManifest> {
        let mut connection = self.pool.acquire().await?;
        let process = find_process(&mut connection, process_id).await?;
        let inv_tsc_frequency = get_process_tick_length_ms(&process);
        let block = find_block(&mut connection, block_id).await?;
        let begin_tick_offset = block.begin_ticks - process.start_ticks;
        let end_tick_offset = block.end_ticks - process.start_ticks;
        let desc = MetricBlockDesc {
            block_id: block.block_id.clone(),
            begin_ticks: begin_tick_offset,
            end_ticks: end_tick_offset,
            begin_time_ms: begin_tick_offset as f64 * inv_tsc_frequency,
            end_time_ms: end_tick_offset as f64 * inv_tsc_frequency,
            stream_id: stream_id.to_string(),
        };
        let payload = fetch_block_payload(
            &mut connection,
            self.blob_storage.clone(),
            block_id.to_string(),
        )
        .await?;
        let stream = find_stream(&mut connection, stream_id).await?;
        let mut metrics = HashMap::<String, MetricDesc>::new();
        parse_block(&stream, &payload, |val| {
            if let Value::Object(obj) = val {
                let metric_desc = obj
                    .get::<Arc<Object>>("desc")
                    .with_context(|| format!("reading desc from {:?}", obj))?;
                let name = metric_desc
                    .get_ref("name")
                    .with_context(|| format!("reading name from {:?}", metric_desc))?
                    .as_str()
                    .with_context(|| "as_str on desc name")?;
                metrics
                    .entry(name.to_owned())
                    .or_insert_with(|| MetricDesc {
                        name: name.to_owned(),
                        unit: metric_desc
                            .get_ref("unit")
                            .unwrap()
                            .as_str()
                            .unwrap()
                            .to_string(),
                    });
            }
            Ok(true)
        })?;
        Ok(MetricBlockManifest {
            desc: Some(desc),
            metrics: metrics.values().cloned().collect(),
        })
    }

    #[allow(clippy::cast_precision_loss)]
    async fn get_raw_block_data(
        &self,
        process_id: &str,
        block_id: &str,
        stream_id: &str,
        metric_name: &str,
    ) -> Result<MetricBlockData> {
        let mut connection = self.pool.acquire().await?;
        let process = find_process(&mut connection, process_id).await?;
        let payload = fetch_block_payload(
            &mut connection,
            self.blob_storage.clone(),
            block_id.to_string(),
        )
        .await?;
        let inv_tsc_frequency = get_process_tick_length_ms(&process);
        let stream = find_stream(&mut connection, stream_id).await?;
        let mut points = vec![];
        parse_block(&stream, &payload, |val| {
            if let Value::Object(obj) = val {
                let metric_desc = obj.get::<Arc<Object>>("desc").unwrap();
                let name = metric_desc.get_ref("name").unwrap().as_str().unwrap();
                if name == metric_name {
                    let time = obj.get::<i64>("time").unwrap();
                    let time_ms = (time - process.start_ticks) as f64 * inv_tsc_frequency;
                    let value = match obj.get_ref("value") {
                        Err(e) => {
                            warn!("no value member in metric object {:?}: {:?}", obj, e);
                            None
                        }
                        Result::Ok(Value::None) => {
                            warn!("no value member in metric object {:?}", obj);
                            None
                        }
                        Result::Ok(Value::String(v)) => {
                            warn!("invalid value for metric: {:?}", *v);
                            None
                        }
                        Result::Ok(Value::Object(o)) => {
                            warn!("invalid value for metric: {:?}", *o);
                            None
                        }
                        Result::Ok(Value::U8(v)) => Some(f64::from(*v)),
                        Result::Ok(Value::U32(v)) => Some(f64::from(*v)),
                        Result::Ok(Value::U64(v)) => Some(*v as f64),
                        Result::Ok(Value::I64(v)) => Some(*v as f64),
                        Result::Ok(Value::F64(v)) => Some(*v),
                    };
                    if let Some(v) = value {
                        points.push(MetricDataPoint { time_ms, value: v });
                    }
                }
            }
            Ok(true)
        })?;
        Ok(MetricBlockData { points, lod: 0 })
    }
}
