//! `BlockProcessor` for OTLP `ResourceSpans` payloads → `otel_spans` rows.
//!
//! Materializes one row per span. `events` and `links` go in plain `Binary` columns
//! carrying JSONB bytes — see plan §"Span events and links as `List<Struct>` vs JSONB"
//! for the rationale.

use super::attrs::{any_value_to_jsonb, attrs_to_jsonb};
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
    BinaryBuilder, BinaryDictionaryBuilder, FixedSizeBinaryBuilder, PrimitiveBuilder,
    StringBuilder, StringDictionaryBuilder,
};
use datafusion::arrow::datatypes::{Int32Type, Int64Type, TimestampNanosecondType};
use datafusion::arrow::record_batch::RecordBatch;
use jsonb::Value as JsonbValue;
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_tracing::prelude::*;
use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use opentelemetry_proto::tonic::trace::v1::{
    ResourceSpans, Span, span as span_proto, status::StatusCode as ProtoStatusCode,
};
use prost::Message;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug)]
pub struct OtelSpansBlockProcessor {}

#[async_trait]
impl BlockProcessor for OtelSpansBlockProcessor {
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

        let resource_spans = ResourceSpans::decode(payload.objects.as_slice())
            .with_context(|| "decoding ResourceSpans proto")?;

        let insert_time_nanos = src_block
            .block
            .insert_time
            .timestamp_nanos_opt()
            .with_context(|| "block.insert_time → nanos")?;
        let mut builder = OtelSpansRowBuilder::new(
            src_block.process.process_id.to_string(),
            src_block.block.stream_id.to_string(),
            src_block.block.block_id.to_string(),
            insert_time_nanos,
            src_block.process.clone(),
        );

        for scope_spans in &resource_spans.scope_spans {
            let scope = scope_spans.scope.as_ref();
            for span in &scope_spans.spans {
                builder.append(span, scope, &scope_spans.schema_url)?;
            }
        }

        builder.finish()
    }
}

/// Per-block accumulator for `otel_spans` rows: owns the column builders, time
/// bounds, and per-block constants so `append` only takes per-span inputs.
struct OtelSpansRowBuilder {
    process_ids: StringDictionaryBuilder<Int32Type>,
    stream_ids: StringDictionaryBuilder<Int32Type>,
    block_ids: StringDictionaryBuilder<Int32Type>,
    insert_times: PrimitiveBuilder<TimestampNanosecondType>,
    exes: StringBuilder,
    usernames: StringBuilder,
    computers: StringBuilder,
    process_properties: BinaryDictionaryBuilder<Int32Type>,
    trace_ids: FixedSizeBinaryBuilder,
    span_ids: FixedSizeBinaryBuilder,
    parent_span_ids: FixedSizeBinaryBuilder,
    start_times: PrimitiveBuilder<TimestampNanosecondType>,
    end_times: PrimitiveBuilder<TimestampNanosecondType>,
    durations: PrimitiveBuilder<Int64Type>,
    names: StringDictionaryBuilder<Int32Type>,
    kinds: StringDictionaryBuilder<Int32Type>,
    statuses: StringDictionaryBuilder<Int32Type>,
    status_messages: StringBuilder,
    properties: BinaryDictionaryBuilder<Int32Type>,
    events: BinaryBuilder,
    links: BinaryBuilder,
    min_time: i64,
    max_time: i64,
    nb_appended: usize,
    process_id_str: String,
    stream_id_str: String,
    block_id_str: String,
    insert_time_nanos: i64,
    process: Arc<ProcessMetadata>,
}

impl OtelSpansRowBuilder {
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
            exes: StringBuilder::new(),
            usernames: StringBuilder::new(),
            computers: StringBuilder::new(),
            process_properties: BinaryDictionaryBuilder::new(),
            trace_ids: FixedSizeBinaryBuilder::new(16),
            span_ids: FixedSizeBinaryBuilder::new(8),
            parent_span_ids: FixedSizeBinaryBuilder::new(8),
            start_times: PrimitiveBuilder::new(),
            end_times: PrimitiveBuilder::new(),
            durations: PrimitiveBuilder::new(),
            names: StringDictionaryBuilder::new(),
            kinds: StringDictionaryBuilder::new(),
            statuses: StringDictionaryBuilder::new(),
            status_messages: StringBuilder::new(),
            properties: BinaryDictionaryBuilder::new(),
            events: BinaryBuilder::new(),
            links: BinaryBuilder::new(),
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
        span: &Span,
        scope: Option<&InstrumentationScope>,
        schema_url: &str,
    ) -> Result<()> {
        let start_nanos = span.start_time_unix_nano as i64;
        let end_nanos = span.end_time_unix_nano as i64;
        if start_nanos == 0 || end_nanos == 0 {
            debug!(
                "OTel span without start/end time, skipping (block={})",
                self.block_id_str
            );
            return Ok(());
        }
        if span.trace_id.len() != 16 || span.span_id.len() != 8 {
            debug!(
                "OTel span with bad trace_id ({}b) / span_id ({}b), skipping",
                span.trace_id.len(),
                span.span_id.len(),
            );
            return Ok(());
        }
        self.min_time = self.min_time.min(start_nanos);
        self.max_time = self.max_time.max(end_nanos);

        self.trace_ids
            .append_value(&span.trace_id)
            .with_context(|| "appending trace_id")?;
        self.span_ids
            .append_value(&span.span_id)
            .with_context(|| "appending span_id")?;
        if span.parent_span_id.len() == 8 {
            self.parent_span_ids
                .append_value(&span.parent_span_id)
                .with_context(|| "appending parent_span_id")?;
        } else {
            self.parent_span_ids.append_null();
        }

        self.process_ids.append_value(&self.process_id_str);
        self.stream_ids.append_value(&self.stream_id_str);
        self.block_ids.append_value(&self.block_id_str);
        self.insert_times.append_value(self.insert_time_nanos);
        self.exes.append_value(&self.process.exe);
        self.usernames.append_value(&self.process.username);
        self.computers.append_value(&self.process.computer);
        self.process_properties
            .append_value(&**self.process.properties);

        self.start_times.append_value(start_nanos);
        self.end_times.append_value(end_nanos);
        self.durations.append_value(end_nanos - start_nanos);
        self.names.append_value(&span.name);
        self.kinds.append_value(span_kind_str(span.kind));
        let (status_code, status_message) = match span.status.as_ref() {
            Some(s) => (proto_status_code_str(s.code), Some(s.message.clone())),
            None => ("UNSET", None),
        };
        self.statuses.append_value(status_code);
        match status_message.as_deref() {
            Some(msg) if !msg.is_empty() => self.status_messages.append_value(msg),
            _ => self.status_messages.append_null(),
        }

        // Properties: span attributes + scope info.
        let mut extras: Vec<(String, JsonbValue<'static>)> = Vec::new();
        if let Some(s) = scope {
            if !s.name.is_empty() {
                extras.push((
                    "otel.scope.name".into(),
                    JsonbValue::String(Cow::Owned(s.name.clone())),
                ));
            }
            if !s.version.is_empty() {
                extras.push((
                    "otel.scope.version".into(),
                    JsonbValue::String(Cow::Owned(s.version.clone())),
                ));
            }
            for kv in &s.attributes {
                if let Some(v) = kv.value.as_ref() {
                    extras.push((format!("otel.scope.attr.{}", kv.key), any_value_to_jsonb(v)));
                }
            }
        }
        if !schema_url.is_empty() {
            extras.push((
                "otel.scope.schema_url".into(),
                JsonbValue::String(Cow::Owned(schema_url.to_string())),
            ));
        }
        let props_jsonb = attrs_to_jsonb(&span.attributes, &extras);
        self.properties.append_value(&props_jsonb);

        // Events as JSONB array.
        let events_array: Vec<JsonbValue<'static>> = span
            .events
            .iter()
            .map(|ev| {
                let mut map: BTreeMap<String, JsonbValue<'static>> = BTreeMap::new();
                map.insert(
                    "time".into(),
                    JsonbValue::Number(jsonb::Number::Int64(ev.time_unix_nano as i64)),
                );
                map.insert(
                    "name".into(),
                    JsonbValue::String(Cow::Owned(ev.name.clone())),
                );
                let mut attrs_map: BTreeMap<String, JsonbValue<'static>> = BTreeMap::new();
                for kv in &ev.attributes {
                    if let Some(v) = kv.value.as_ref() {
                        attrs_map.insert(kv.key.clone(), any_value_to_jsonb(v));
                    }
                }
                map.insert("attributes".into(), JsonbValue::Object(attrs_map));
                JsonbValue::Object(map)
            })
            .collect();
        let mut events_bytes = Vec::new();
        JsonbValue::Array(events_array).write_to_vec(&mut events_bytes);
        self.events.append_value(&events_bytes);

        // Links as JSONB array.
        let links_array: Vec<JsonbValue<'static>> = span
            .links
            .iter()
            .map(|link| {
                let mut map: BTreeMap<String, JsonbValue<'static>> = BTreeMap::new();
                map.insert(
                    "trace_id".into(),
                    JsonbValue::String(Cow::Owned(hex::encode(&link.trace_id))),
                );
                map.insert(
                    "span_id".into(),
                    JsonbValue::String(Cow::Owned(hex::encode(&link.span_id))),
                );
                let mut attrs_map: BTreeMap<String, JsonbValue<'static>> = BTreeMap::new();
                for kv in &link.attributes {
                    if let Some(v) = kv.value.as_ref() {
                        attrs_map.insert(kv.key.clone(), any_value_to_jsonb(v));
                    }
                }
                map.insert("attributes".into(), JsonbValue::Object(attrs_map));
                JsonbValue::Object(map)
            })
            .collect();
        let mut links_bytes = Vec::new();
        JsonbValue::Array(links_array).write_to_vec(&mut links_bytes);
        self.links.append_value(&links_bytes);

        self.nb_appended += 1;
        Ok(())
    }

    fn finish(mut self) -> Result<Option<PartitionRowSet>> {
        if self.nb_appended == 0 {
            return Ok(None);
        }
        let schema = Arc::new(crate::lakehouse::otel::spans_table::otel_spans_table_schema());
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
                Arc::new(self.process_properties.finish()),
                Arc::new(self.trace_ids.finish()),
                Arc::new(self.span_ids.finish()),
                Arc::new(self.parent_span_ids.finish()),
                Arc::new(self.start_times.finish().with_timezone_utc()),
                Arc::new(self.end_times.finish().with_timezone_utc()),
                Arc::new(self.durations.finish()),
                Arc::new(self.names.finish()),
                Arc::new(self.kinds.finish()),
                Arc::new(self.statuses.finish()),
                Arc::new(self.status_messages.finish()),
                Arc::new(self.properties.finish()),
                Arc::new(self.events.finish()),
                Arc::new(self.links.finish()),
            ],
        )
        .with_context(|| "building otel_spans batch")?;

        Ok(Some(PartitionRowSet::new(
            TimeRange::new(
                DateTime::from_timestamp_nanos(self.min_time),
                DateTime::from_timestamp_nanos(self.max_time),
            ),
            batch,
        )))
    }
}

fn span_kind_str(kind: i32) -> &'static str {
    match span_proto::SpanKind::try_from(kind).unwrap_or(span_proto::SpanKind::Unspecified) {
        span_proto::SpanKind::Unspecified => "UNSPECIFIED",
        span_proto::SpanKind::Internal => "INTERNAL",
        span_proto::SpanKind::Server => "SERVER",
        span_proto::SpanKind::Client => "CLIENT",
        span_proto::SpanKind::Producer => "PRODUCER",
        span_proto::SpanKind::Consumer => "CONSUMER",
    }
}

fn proto_status_code_str(code: i32) -> &'static str {
    match ProtoStatusCode::try_from(code).unwrap_or(ProtoStatusCode::Unset) {
        ProtoStatusCode::Unset => "UNSET",
        ProtoStatusCode::Ok => "OK",
        ProtoStatusCode::Error => "ERROR",
    }
}
