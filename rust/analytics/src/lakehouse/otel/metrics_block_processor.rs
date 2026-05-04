//! `BlockProcessor` for OTLP `ResourceMetrics` payloads → `measures` rows.
//!
//! Handles Sum and Gauge data points; logs and skips Histogram, ExponentialHistogram,
//! and Summary (deferred to v2 — see plan §"Histograms deferred"). Aggregation
//! temporality and `is_monotonic` ride along on per-row properties.

use super::attrs::attrs_to_jsonb;
use crate::lakehouse::{
    block_partition_spec::BlockProcessor, partition_source_data::PartitionSourceBlock,
    write_partition::PartitionRowSet,
};
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
        let insert_time_nanos = src_block
            .block
            .insert_time
            .timestamp_nanos_opt()
            .with_context(|| "block.insert_time → nanos")?;
        let mut builder = MeasuresRowBuilder::new(
            process.process_id.to_string(),
            src_block.block.stream_id.to_string(),
            src_block.block.block_id.to_string(),
            insert_time_nanos,
            process,
        );

        for scope_metrics in &resource_metrics.scope_metrics {
            let scope = scope_metrics.scope.as_ref();
            let scope_name = scope.map(|s| s.name.clone()).unwrap_or_default();

            for metric in &scope_metrics.metrics {
                match metric.data.as_ref() {
                    Some(Data::Sum(sum)) => {
                        let extras = [
                            (
                                "otel.metric.aggregation_temporality".to_string(),
                                JsonbValue::Number(jsonb::Number::Int64(
                                    sum.aggregation_temporality as i64,
                                )),
                            ),
                            (
                                "otel.metric.is_monotonic".to_string(),
                                JsonbValue::Bool(sum.is_monotonic),
                            ),
                            (
                                "otel.metric.kind".to_string(),
                                JsonbValue::String(Cow::Borrowed("sum")),
                            ),
                        ];
                        for dp in &sum.data_points {
                            builder.append(&scope_name, &metric.name, &metric.unit, dp, &extras);
                        }
                    }
                    Some(Data::Gauge(gauge)) => {
                        let extras = [(
                            "otel.metric.kind".to_string(),
                            JsonbValue::String(Cow::Borrowed("gauge")),
                        )];
                        for dp in &gauge.data_points {
                            builder.append(&scope_name, &metric.name, &metric.unit, dp, &extras);
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
            }
        }

        builder.finish()
    }
}

/// Per-block accumulator for `measures` rows: owns the column builders, time
/// bounds, and per-block constants so `append` only takes per-data-point inputs.
struct MeasuresRowBuilder<'a> {
    process_ids: StringDictionaryBuilder<Int16Type>,
    stream_ids: StringDictionaryBuilder<Int16Type>,
    block_ids: StringDictionaryBuilder<Int16Type>,
    insert_times: PrimitiveBuilder<TimestampNanosecondType>,
    exes: StringDictionaryBuilder<Int16Type>,
    usernames: StringDictionaryBuilder<Int16Type>,
    computers: StringDictionaryBuilder<Int16Type>,
    times: PrimitiveBuilder<TimestampNanosecondType>,
    targets: StringDictionaryBuilder<Int16Type>,
    names: StringDictionaryBuilder<Int16Type>,
    units: StringDictionaryBuilder<Int16Type>,
    values: PrimitiveBuilder<Float64Type>,
    properties: BinaryDictionaryBuilder<Int32Type>,
    process_properties: BinaryDictionaryBuilder<Int32Type>,
    min_time: i64,
    max_time: i64,
    nb_appended: usize,
    process_id_str: String,
    stream_id_str: String,
    block_id_str: String,
    insert_time_nanos: i64,
    process: &'a crate::metadata::ProcessMetadata,
}

impl<'a> MeasuresRowBuilder<'a> {
    fn new(
        process_id_str: String,
        stream_id_str: String,
        block_id_str: String,
        insert_time_nanos: i64,
        process: &'a crate::metadata::ProcessMetadata,
    ) -> Self {
        Self {
            process_ids: StringDictionaryBuilder::new(),
            stream_ids: StringDictionaryBuilder::new(),
            block_ids: StringDictionaryBuilder::new(),
            insert_times: PrimitiveBuilder::new(),
            exes: StringDictionaryBuilder::new(),
            usernames: StringDictionaryBuilder::new(),
            computers: StringDictionaryBuilder::new(),
            times: PrimitiveBuilder::new(),
            targets: StringDictionaryBuilder::new(),
            names: StringDictionaryBuilder::new(),
            units: StringDictionaryBuilder::new(),
            values: PrimitiveBuilder::new(),
            properties: BinaryDictionaryBuilder::new(),
            process_properties: BinaryDictionaryBuilder::new(),
            min_time: i64::MAX,
            max_time: i64::MIN,
            nb_appended: 0,
            process_id_str,
            stream_id_str,
            block_id_str,
            insert_time_nanos,
            process,
        }
    }

    fn append(
        &mut self,
        scope_name: &str,
        metric_name: &str,
        unit: &str,
        dp: &opentelemetry_proto::tonic::metrics::v1::NumberDataPoint,
        extras: &[(String, JsonbValue<'static>)],
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

        self.min_time = self.min_time.min(time_nanos);
        self.max_time = self.max_time.max(time_nanos);

        let props_jsonb = attrs_to_jsonb(&dp.attributes, extras);

        self.process_ids.append_value(&self.process_id_str);
        self.stream_ids.append_value(&self.stream_id_str);
        self.block_ids.append_value(&self.block_id_str);
        self.insert_times.append_value(self.insert_time_nanos);
        self.exes.append_value(&self.process.exe);
        self.usernames.append_value(&self.process.username);
        self.computers.append_value(&self.process.computer);
        self.times.append_value(time_nanos);
        self.targets.append_value(scope_name);
        self.names.append_value(metric_name);
        self.units.append_value(unit);
        self.values.append_value(value);
        self.properties.append_value(&props_jsonb);
        self.process_properties
            .append_value(&**self.process.properties);

        self.nb_appended += 1;
    }

    fn finish(mut self) -> Result<Option<PartitionRowSet>> {
        if self.nb_appended == 0 {
            return Ok(None);
        }
        let schema = Arc::new(crate::metrics_table::metrics_table_schema());
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(self.process_ids.finish()),
                Arc::new(self.stream_ids.finish()),
                Arc::new(self.block_ids.finish()),
                Arc::new(self.insert_times.finish().with_timezone_utc()),
                Arc::new(self.exes.finish()),
                Arc::new(self.usernames.finish()),
                Arc::new(self.computers.finish()),
                Arc::new(self.times.finish().with_timezone_utc()),
                Arc::new(self.targets.finish()),
                Arc::new(self.names.finish()),
                Arc::new(self.units.finish()),
                Arc::new(self.values.finish()),
                Arc::new(self.properties.finish()),
                Arc::new(self.process_properties.finish()),
            ],
        )
        .with_context(|| "building OTel measures batch")?;

        Ok(Some(PartitionRowSet::new(
            TimeRange::new(
                DateTime::from_timestamp_nanos(self.min_time),
                DateTime::from_timestamp_nanos(self.max_time),
            ),
            batch,
        )))
    }
}
