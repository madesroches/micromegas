//! `BlockProcessor` for OTLP `ResourceMetrics` payloads → `measures` rows.
//!
//! Handles Sum and Gauge data points; logs and skips Histogram, ExponentialHistogram,
//! and Summary (deferred to v2 — see plan §"Histograms deferred"). Aggregation
//! temporality and `is_monotonic` ride along on per-row properties.

use super::{
    block_partition_spec::BlockProcessor, partition_source_data::PartitionSourceBlock,
    write_partition::PartitionRowSet,
};
use crate::lakehouse::otel_attrs::{any_value_to_jsonb, attrs_to_jsonb};
use crate::payload::fetch_block_payload;
use crate::time::TimeRange;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::DateTime;
use datafusion::arrow::array::{
    BinaryDictionaryBuilder, PrimitiveBuilder, StringDictionaryBuilder,
};
use datafusion::arrow::datatypes::{Float64Type, Int16Type, Int32Type, TimestampNanosecondType};
use datafusion::arrow::record_batch::RecordBatch;
use jsonb::Value as JsonbValue;
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_tracing::prelude::*;
use opentelemetry_proto::tonic::common::v1::KeyValue;
use opentelemetry_proto::tonic::metrics::v1::{ResourceMetrics, metric::Data, number_data_point};
use prost::Message;
use std::borrow::Cow;
use std::sync::Arc;

#[derive(Debug)]
pub struct OtelMetricsBlockProcessor {}

#[async_trait]
impl BlockProcessor for OtelMetricsBlockProcessor {
    #[span_fn]
    async fn process(
        &self,
        blob_storage: Arc<BlobStorage>,
        src_block: Arc<PartitionSourceBlock>,
    ) -> Result<Option<PartitionRowSet>> {
        let payload = fetch_block_payload(
            blob_storage,
            sqlx::types::Uuid::from_bytes(*src_block.block.process_id.as_bytes()),
            sqlx::types::Uuid::from_bytes(*src_block.block.stream_id.as_bytes()),
            sqlx::types::Uuid::from_bytes(*src_block.block.block_id.as_bytes()),
        )
        .await
        .with_context(|| "fetch_block_payload")?;

        let resource_metrics = ResourceMetrics::decode(payload.objects.as_slice())
            .with_context(|| "decoding ResourceMetrics proto")?;

        let process = &src_block.process;
        let stream_id_str = src_block.block.stream_id.to_string();
        let block_id_str = src_block.block.block_id.to_string();
        let process_id_str = process.process_id.to_string();
        let insert_time_nanos = src_block
            .block
            .insert_time
            .timestamp_nanos_opt()
            .with_context(|| "block.insert_time → nanos")?;

        let mut process_ids = StringDictionaryBuilder::<Int16Type>::new();
        let mut stream_ids = StringDictionaryBuilder::<Int16Type>::new();
        let mut block_ids = StringDictionaryBuilder::<Int16Type>::new();
        let mut insert_times = PrimitiveBuilder::<TimestampNanosecondType>::new();
        let mut exes = StringDictionaryBuilder::<Int16Type>::new();
        let mut usernames = StringDictionaryBuilder::<Int16Type>::new();
        let mut computers = StringDictionaryBuilder::<Int16Type>::new();
        let mut times = PrimitiveBuilder::<TimestampNanosecondType>::new();
        let mut targets = StringDictionaryBuilder::<Int16Type>::new();
        let mut names = StringDictionaryBuilder::<Int16Type>::new();
        let mut units = StringDictionaryBuilder::<Int16Type>::new();
        let mut values = PrimitiveBuilder::<Float64Type>::new();
        let mut properties = BinaryDictionaryBuilder::<Int32Type>::new();
        let mut process_properties = BinaryDictionaryBuilder::<Int32Type>::new();

        let mut min_time = i64::MAX;
        let mut max_time = i64::MIN;
        let mut nb_appended = 0usize;

        for scope_metrics in &resource_metrics.scope_metrics {
            let scope = scope_metrics.scope.as_ref();
            let scope_name = scope.map(|s| s.name.clone()).unwrap_or_default();

            for metric in &scope_metrics.metrics {
                match metric.data.as_ref() {
                    Some(Data::Sum(sum)) => {
                        let aggregation_temporality = sum.aggregation_temporality;
                        let is_monotonic = sum.is_monotonic;
                        for dp in &sum.data_points {
                            append_number_data_point(
                                &mut process_ids,
                                &mut stream_ids,
                                &mut block_ids,
                                &mut insert_times,
                                &mut exes,
                                &mut usernames,
                                &mut computers,
                                &mut times,
                                &mut targets,
                                &mut names,
                                &mut units,
                                &mut values,
                                &mut properties,
                                &mut process_properties,
                                &process_id_str,
                                &stream_id_str,
                                &block_id_str,
                                insert_time_nanos,
                                process,
                                &scope_name,
                                &metric.name,
                                &metric.unit,
                                dp,
                                &dp.attributes,
                                &[
                                    (
                                        "otel.metric.aggregation_temporality".to_string(),
                                        JsonbValue::Number(jsonb::Number::Int64(
                                            aggregation_temporality as i64,
                                        )),
                                    ),
                                    (
                                        "otel.metric.is_monotonic".to_string(),
                                        JsonbValue::Bool(is_monotonic),
                                    ),
                                    (
                                        "otel.metric.kind".to_string(),
                                        JsonbValue::String(Cow::Borrowed("sum")),
                                    ),
                                ],
                                &mut min_time,
                                &mut max_time,
                                &mut nb_appended,
                            );
                        }
                    }
                    Some(Data::Gauge(gauge)) => {
                        for dp in &gauge.data_points {
                            append_number_data_point(
                                &mut process_ids,
                                &mut stream_ids,
                                &mut block_ids,
                                &mut insert_times,
                                &mut exes,
                                &mut usernames,
                                &mut computers,
                                &mut times,
                                &mut targets,
                                &mut names,
                                &mut units,
                                &mut values,
                                &mut properties,
                                &mut process_properties,
                                &process_id_str,
                                &stream_id_str,
                                &block_id_str,
                                insert_time_nanos,
                                process,
                                &scope_name,
                                &metric.name,
                                &metric.unit,
                                dp,
                                &dp.attributes,
                                &[(
                                    "otel.metric.kind".to_string(),
                                    JsonbValue::String(Cow::Borrowed("gauge")),
                                )],
                                &mut min_time,
                                &mut max_time,
                                &mut nb_appended,
                            );
                        }
                    }
                    Some(Data::Histogram(h)) => {
                        debug!(
                            "OTel histogram dropped (deferred to v2): name={} unit={} points={}",
                            metric.name,
                            metric.unit,
                            h.data_points.len()
                        );
                    }
                    Some(Data::ExponentialHistogram(h)) => {
                        debug!(
                            "OTel exponential_histogram dropped (deferred to v2): name={} unit={} points={}",
                            metric.name,
                            metric.unit,
                            h.data_points.len()
                        );
                    }
                    Some(Data::Summary(s)) => {
                        debug!(
                            "OTel summary dropped (deprecated in OTel): name={} unit={} points={}",
                            metric.name,
                            metric.unit,
                            s.data_points.len()
                        );
                    }
                    None => {}
                }
                let _ = any_value_to_jsonb; // keep helper visible to future exemplar extraction
            }
        }

        if nb_appended == 0 {
            return Ok(None);
        }

        let schema = Arc::new(crate::metrics_table::metrics_table_schema());
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(process_ids.finish()),
                Arc::new(stream_ids.finish()),
                Arc::new(block_ids.finish()),
                Arc::new(insert_times.finish().with_timezone_utc()),
                Arc::new(exes.finish()),
                Arc::new(usernames.finish()),
                Arc::new(computers.finish()),
                Arc::new(times.finish().with_timezone_utc()),
                Arc::new(targets.finish()),
                Arc::new(names.finish()),
                Arc::new(units.finish()),
                Arc::new(values.finish()),
                Arc::new(properties.finish()),
                Arc::new(process_properties.finish()),
            ],
        )
        .with_context(|| "building OTel measures batch")?;

        Ok(Some(PartitionRowSet::new(
            TimeRange::new(
                DateTime::from_timestamp_nanos(min_time),
                DateTime::from_timestamp_nanos(max_time),
            ),
            batch,
        )))
    }
}

#[allow(clippy::too_many_arguments)]
fn append_number_data_point(
    process_ids: &mut StringDictionaryBuilder<Int16Type>,
    stream_ids: &mut StringDictionaryBuilder<Int16Type>,
    block_ids: &mut StringDictionaryBuilder<Int16Type>,
    insert_times: &mut PrimitiveBuilder<TimestampNanosecondType>,
    exes: &mut StringDictionaryBuilder<Int16Type>,
    usernames: &mut StringDictionaryBuilder<Int16Type>,
    computers: &mut StringDictionaryBuilder<Int16Type>,
    times: &mut PrimitiveBuilder<TimestampNanosecondType>,
    targets: &mut StringDictionaryBuilder<Int16Type>,
    names: &mut StringDictionaryBuilder<Int16Type>,
    units: &mut StringDictionaryBuilder<Int16Type>,
    values: &mut PrimitiveBuilder<Float64Type>,
    properties: &mut BinaryDictionaryBuilder<Int32Type>,
    process_properties: &mut BinaryDictionaryBuilder<Int32Type>,
    process_id_str: &str,
    stream_id_str: &str,
    block_id_str: &str,
    insert_time_nanos: i64,
    process: &crate::metadata::ProcessMetadata,
    scope_name: &str,
    metric_name: &str,
    unit: &str,
    dp: &opentelemetry_proto::tonic::metrics::v1::NumberDataPoint,
    attrs: &[KeyValue],
    extras: &[(String, JsonbValue<'static>)],
    min_time: &mut i64,
    max_time: &mut i64,
    nb_appended: &mut usize,
) {
    let time_nanos = dp.time_unix_nano as i64;
    if time_nanos == 0 {
        debug!("OTel metric data point for {metric_name} dropped (time_unix_nano=0)",);
        return;
    }

    let value = match dp.value.as_ref() {
        Some(number_data_point::Value::AsDouble(d)) => *d,
        Some(number_data_point::Value::AsInt(i)) => *i as f64,
        None => {
            debug!("OTel data point for {metric_name} has no value, skipping");
            return;
        }
    };

    *min_time = (*min_time).min(time_nanos);
    *max_time = (*max_time).max(time_nanos);

    let props_jsonb = attrs_to_jsonb(attrs, extras);

    process_ids.append_value(process_id_str);
    stream_ids.append_value(stream_id_str);
    block_ids.append_value(block_id_str);
    insert_times.append_value(insert_time_nanos);
    exes.append_value(&process.exe);
    usernames.append_value(&process.username);
    computers.append_value(&process.computer);
    times.append_value(time_nanos);
    targets.append_value(scope_name);
    names.append_value(metric_name);
    units.append_value(unit);
    values.append_value(value);
    properties.append_value(&props_jsonb);
    process_properties.append_value(&**process.properties);

    *nb_appended += 1;
}
