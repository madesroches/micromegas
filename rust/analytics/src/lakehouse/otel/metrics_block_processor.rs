//! `BlockProcessor` for OTLP `ResourceMetrics` payloads → `measures` rows.
//!
//! Handles Sum and Gauge data points directly, and Summary data points by fanning each
//! one out into count/sum/min/max rows under suffixed metric names (`<metric>_count`,
//! `_sum`, `_min`, `_max`); any other `quantile_values` entry (configured percentiles)
//! is logged and dropped. Histogram and ExponentialHistogram are still logged and
//! skipped — a histogram-aware schema is future work. Aggregation temporality and
//! `is_monotonic` ride along on per-row properties for Sum/Gauge.

use super::attrs::attrs_to_jsonb;
use crate::lakehouse::{
    block_partition_spec::BlockProcessor, partition_source_data::PartitionSourceBlock,
    write_partition::PartitionRowSet,
};
use crate::metadata::ProcessMetadata;
use crate::payload::fetch_block_payload;
use crate::time::TimeRange;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::DateTime;
use datafusion::arrow::array::{
    BinaryDictionaryBuilder, PrimitiveBuilder, StringDictionaryBuilder,
};
use datafusion::arrow::datatypes::{Float64Type, Int32Type, TimestampNanosecondType};
use datafusion::arrow::record_batch::RecordBatch;
use jsonb::Value as JsonbValue;
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_tracing::prelude::*;
use opentelemetry_proto::tonic::metrics::v1::{
    NumberDataPoint, ResourceMetrics, SummaryDataPoint, metric::Data, number_data_point,
};
use prost::Message;
use std::borrow::Cow;
use std::sync::Arc;

/// Returns `time_unix_nano` as `i64` unless it's zero, in which case the data point is
/// logged and skipped. Shared by `append` (Sum/Gauge) and `append_summary`, which only
/// differ in the `kind` word used in the log message.
fn nonzero_time_nanos(kind: &str, metric_name: &str, time_unix_nano: u64) -> Option<i64> {
    let time_nanos = time_unix_nano as i64;
    if time_nanos == 0 {
        debug!("OTel {kind} data point for {metric_name} dropped (time_unix_nano=0)");
        return None;
    }
    Some(time_nanos)
}

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

        let insert_time_nanos = src_block
            .block
            .insert_time
            .timestamp_nanos_opt()
            .with_context(|| "block.insert_time → nanos")?;
        let mut builder = MeasuresRowBuilder::new(
            src_block.process.process_id.to_string(),
            src_block.block.stream_id.to_string(),
            src_block.block.block_id.to_string(),
            insert_time_nanos,
            src_block.process.clone(),
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
                            builder.append(&scope_name, &metric.name, &metric.unit, dp, &extras)?;
                        }
                    }
                    Some(Data::Gauge(gauge)) => {
                        let extras = [(
                            "otel.metric.kind".to_string(),
                            JsonbValue::String(Cow::Borrowed("gauge")),
                        )];
                        for dp in &gauge.data_points {
                            builder.append(&scope_name, &metric.name, &metric.unit, dp, &extras)?;
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
                        for dp in &s.data_points {
                            builder.append_summary(&scope_name, &metric.name, &metric.unit, dp)?;
                        }
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
struct MeasuresRowBuilder {
    process_ids: StringDictionaryBuilder<Int32Type>,
    stream_ids: StringDictionaryBuilder<Int32Type>,
    block_ids: StringDictionaryBuilder<Int32Type>,
    insert_times: PrimitiveBuilder<TimestampNanosecondType>,
    exes: StringDictionaryBuilder<Int32Type>,
    usernames: StringDictionaryBuilder<Int32Type>,
    computers: StringDictionaryBuilder<Int32Type>,
    times: PrimitiveBuilder<TimestampNanosecondType>,
    targets: StringDictionaryBuilder<Int32Type>,
    names: StringDictionaryBuilder<Int32Type>,
    units: StringDictionaryBuilder<Int32Type>,
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
    process: Arc<ProcessMetadata>,
}

impl MeasuresRowBuilder {
    fn new(
        process_id_str: String,
        stream_id_str: String,
        block_id_str: String,
        insert_time_nanos: i64,
        process: Arc<ProcessMetadata>,
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

    /// Appends one `measures` row for an already-extracted point. Shared tail of
    /// `append` (Sum/Gauge) and `append_summary` (Summary). `props_jsonb` is the
    /// already-serialized `properties` payload — attributes and derived extras
    /// merged by `attrs_to_jsonb` — so a caller fanning one data point out into
    /// several rows serializes it once.
    fn append_row(
        &mut self,
        scope_name: &str,
        metric_name: &str,
        unit: &str,
        time_nanos: i64,
        value: f64,
        props_jsonb: &[u8],
    ) -> Result<()> {
        self.min_time = self.min_time.min(time_nanos);
        self.max_time = self.max_time.max(time_nanos);

        self.process_ids.append(&self.process_id_str)?;
        self.stream_ids.append(&self.stream_id_str)?;
        self.block_ids.append(&self.block_id_str)?;
        self.insert_times.append_value(self.insert_time_nanos);
        self.exes.append(&self.process.exe)?;
        self.usernames.append(&self.process.username)?;
        self.computers.append(&self.process.computer)?;
        self.times.append_value(time_nanos);
        self.targets.append(scope_name)?;
        self.names.append(metric_name)?;
        self.units.append(unit)?;
        self.values.append_value(value);
        self.properties.append(props_jsonb)?;
        self.process_properties.append(&**self.process.properties)?;

        self.nb_appended += 1;
        Ok(())
    }

    fn append(
        &mut self,
        scope_name: &str,
        metric_name: &str,
        unit: &str,
        dp: &NumberDataPoint,
        extras: &[(String, JsonbValue<'static>)],
    ) -> Result<()> {
        let Some(time_nanos) = nonzero_time_nanos("metric", metric_name, dp.time_unix_nano) else {
            return Ok(());
        };

        let value = match dp.value.as_ref() {
            Some(number_data_point::Value::AsDouble(d)) => *d,
            Some(number_data_point::Value::AsInt(i)) => *i as f64,
            None => {
                debug!("OTel data point for {metric_name} has no value, skipping");
                return Ok(());
            }
        };

        self.append_row(
            scope_name,
            metric_name,
            unit,
            time_nanos,
            value,
            &attrs_to_jsonb(&dp.attributes, extras),
        )
    }

    /// Fans a `SummaryDataPoint` out into rows for the four fixed statistics
    /// (count, sum, min, max), each under its own suffixed metric name. Any
    /// `quantile_values` entry other than `q=0.0`/`q=1.0` is logged and dropped —
    /// configured percentiles are out of scope. No derived `otel.metric.*` extras
    /// (aggregation_temporality/is_monotonic/kind) are added for Summary rows; the
    /// per-point `dp.attributes` still populate `properties` the same as Sum/Gauge.
    fn append_summary(
        &mut self,
        scope_name: &str,
        metric_name: &str,
        unit: &str,
        dp: &SummaryDataPoint,
    ) -> Result<()> {
        let Some(time_nanos) = nonzero_time_nanos("summary", metric_name, dp.time_unix_nano) else {
            return Ok(());
        };

        // All four statistics share the same attribute map, so serialize it once.
        let props_jsonb = attrs_to_jsonb(&dp.attributes, &[]);

        self.append_row(
            scope_name,
            &format!("{metric_name}_count"),
            "",
            time_nanos,
            dp.count as f64,
            &props_jsonb,
        )?;
        self.append_row(
            scope_name,
            &format!("{metric_name}_sum"),
            unit,
            time_nanos,
            dp.sum,
            &props_jsonb,
        )?;

        let mut min_emitted = false;
        let mut max_emitted = false;
        for q in &dp.quantile_values {
            if q.quantile == 0.0 {
                if min_emitted {
                    debug!(
                        "OTel summary quantile dropped (duplicate q=0.0, _min already emitted): \
                         name={metric_name}"
                    );
                    continue;
                }
                min_emitted = true;
                self.append_row(
                    scope_name,
                    &format!("{metric_name}_min"),
                    unit,
                    time_nanos,
                    q.value,
                    &props_jsonb,
                )?;
            } else if q.quantile == 1.0 {
                if max_emitted {
                    debug!(
                        "OTel summary quantile dropped (duplicate q=1.0, _max already emitted): \
                         name={metric_name}"
                    );
                    continue;
                }
                max_emitted = true;
                self.append_row(
                    scope_name,
                    &format!("{metric_name}_max"),
                    unit,
                    time_nanos,
                    q.value,
                    &props_jsonb,
                )?;
            } else {
                debug!(
                    "OTel summary quantile dropped (only count/sum/min/max are materialized): \
                     name={metric_name} quantile={}",
                    q.quantile
                );
            }
        }
        Ok(())
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
