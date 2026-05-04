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
use opentelemetry_proto::tonic::trace::v1::{
    ResourceSpans, span as span_proto, status::StatusCode as ProtoStatusCode,
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

        let process = &src_block.process;
        let stream_id_str = src_block.block.stream_id.to_string();
        let block_id_str = src_block.block.block_id.to_string();
        let process_id_str = process.process_id.to_string();
        let insert_time_nanos = src_block
            .block
            .insert_time
            .timestamp_nanos_opt()
            .with_context(|| "block.insert_time → nanos")?;

        let mut process_ids = StringDictionaryBuilder::<Int32Type>::new();
        let mut stream_ids = StringDictionaryBuilder::<Int32Type>::new();
        let mut block_ids = StringDictionaryBuilder::<Int32Type>::new();
        let mut insert_times = PrimitiveBuilder::<TimestampNanosecondType>::new();
        let mut exes = StringBuilder::new();
        let mut usernames = StringBuilder::new();
        let mut computers = StringBuilder::new();
        let mut process_properties = BinaryDictionaryBuilder::<Int32Type>::new();
        let mut trace_ids = FixedSizeBinaryBuilder::new(16);
        let mut span_ids = FixedSizeBinaryBuilder::new(8);
        let mut parent_span_ids = FixedSizeBinaryBuilder::new(8);
        let mut start_times = PrimitiveBuilder::<TimestampNanosecondType>::new();
        let mut end_times = PrimitiveBuilder::<TimestampNanosecondType>::new();
        let mut durations = PrimitiveBuilder::<Int64Type>::new();
        let mut names = StringDictionaryBuilder::<Int32Type>::new();
        let mut kinds = StringDictionaryBuilder::<Int32Type>::new();
        let mut statuses = StringDictionaryBuilder::<Int32Type>::new();
        let mut status_messages = StringBuilder::new();
        let mut properties = BinaryDictionaryBuilder::<Int32Type>::new();
        let mut events = BinaryBuilder::new();
        let mut links = BinaryBuilder::new();

        let mut min_time = i64::MAX;
        let mut max_time = i64::MIN;
        let mut nb_appended = 0usize;

        for scope_spans in &resource_spans.scope_spans {
            let scope = scope_spans.scope.as_ref();
            for span in &scope_spans.spans {
                let start_nanos = span.start_time_unix_nano as i64;
                let end_nanos = span.end_time_unix_nano as i64;
                if start_nanos == 0 || end_nanos == 0 {
                    debug!("OTel span without start/end time, skipping (block={block_id_str})");
                    continue;
                }
                if span.trace_id.len() != 16 || span.span_id.len() != 8 {
                    debug!(
                        "OTel span with bad trace_id ({}b) / span_id ({}b), skipping",
                        span.trace_id.len(),
                        span.span_id.len(),
                    );
                    continue;
                }
                min_time = min_time.min(start_nanos);
                max_time = max_time.max(end_nanos);

                trace_ids
                    .append_value(&span.trace_id)
                    .with_context(|| "appending trace_id")?;
                span_ids
                    .append_value(&span.span_id)
                    .with_context(|| "appending span_id")?;
                if span.parent_span_id.len() == 8 {
                    parent_span_ids
                        .append_value(&span.parent_span_id)
                        .with_context(|| "appending parent_span_id")?;
                } else {
                    parent_span_ids.append_null();
                }

                process_ids.append_value(&process_id_str);
                stream_ids.append_value(&stream_id_str);
                block_ids.append_value(&block_id_str);
                insert_times.append_value(insert_time_nanos);
                exes.append_value(&process.exe);
                usernames.append_value(&process.username);
                computers.append_value(&process.computer);
                process_properties.append_value(&**process.properties);

                start_times.append_value(start_nanos);
                end_times.append_value(end_nanos);
                durations.append_value(end_nanos - start_nanos);
                names.append_value(&span.name);
                kinds.append_value(span_kind_str(span.kind));
                let (status_code, status_message) = match span.status.as_ref() {
                    Some(s) => (proto_status_code_str(s.code), Some(s.message.clone())),
                    None => ("UNSET", None),
                };
                statuses.append_value(status_code);
                match status_message.as_deref() {
                    Some(msg) if !msg.is_empty() => status_messages.append_value(msg),
                    _ => status_messages.append_null(),
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
                            extras.push((
                                format!("otel.scope.attr.{}", kv.key),
                                any_value_to_jsonb(v),
                            ));
                        }
                    }
                }
                if !scope_spans.schema_url.is_empty() {
                    extras.push((
                        "otel.scope.schema_url".into(),
                        JsonbValue::String(Cow::Owned(scope_spans.schema_url.clone())),
                    ));
                }
                let props_jsonb = attrs_to_jsonb(&span.attributes, &extras);
                properties.append_value(&props_jsonb);

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
                events.append_value(&events_bytes);

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
                links.append_value(&links_bytes);

                nb_appended += 1;
            }
        }

        if nb_appended == 0 {
            return Ok(None);
        }

        let schema = Arc::new(crate::lakehouse::otel::spans_table::otel_spans_table_schema());
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
                Arc::new(process_properties.finish()),
                Arc::new(trace_ids.finish()),
                Arc::new(span_ids.finish()),
                Arc::new(parent_span_ids.finish()),
                Arc::new(start_times.finish().with_timezone_utc()),
                Arc::new(end_times.finish().with_timezone_utc()),
                Arc::new(durations.finish()),
                Arc::new(names.finish()),
                Arc::new(kinds.finish()),
                Arc::new(statuses.finish()),
                Arc::new(status_messages.finish()),
                Arc::new(properties.finish()),
                Arc::new(events.finish()),
                Arc::new(links.finish()),
            ],
        )
        .with_context(|| "building otel_spans batch")?;

        Ok(Some(PartitionRowSet::new(
            TimeRange::new(
                DateTime::from_timestamp_nanos(min_time),
                DateTime::from_timestamp_nanos(max_time),
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
