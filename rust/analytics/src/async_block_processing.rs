use crate::{payload::parse_block, scope::ScopeDesc};
use anyhow::{Context, Result};
use micromegas_telemetry::{block_wire_format::BlockPayload, stream_info::StreamInfo};
use micromegas_tracing::prelude::*;
use micromegas_transit::value::{Object, Value};
use std::sync::Arc;

/// Helper function to extract async event fields (non-named)
fn on_async_event<F>(obj: &Object, mut fun: F) -> Result<bool>
where
    F: FnMut(Arc<Object>, u64, u64, i64) -> Result<bool>,
{
    let span_id = obj.get::<u64>("span_id")?;
    let parent_span_id = obj.get::<u64>("parent_span_id")?;
    let time = obj.get::<i64>("time")?;
    let span_desc = obj.get::<Arc<Object>>("span_desc")?;
    fun(span_desc, span_id, parent_span_id, time)
}

/// Helper function to extract async named event fields  
fn on_async_named_event<F>(obj: &Object, mut fun: F) -> Result<bool>
where
    F: FnMut(Arc<Object>, Arc<String>, u64, u64, i64) -> Result<bool>,
{
    let span_id = obj.get::<u64>("span_id")?;
    let parent_span_id = obj.get::<u64>("parent_span_id")?;
    let time = obj.get::<i64>("time")?;
    let span_location = obj.get::<Arc<Object>>("span_location")?;
    let name = obj.get::<Arc<String>>("name")?;
    fun(span_location, name, span_id, parent_span_id, time)
}

/// Trait for processing async event blocks.
pub trait AsyncBlockProcessor {
    fn on_begin_async_scope(
        &mut self,
        block_id: &str,
        scope: ScopeDesc,
        ts: i64,
        span_id: i64,
        parent_span_id: i64,
    ) -> Result<bool>;
    fn on_end_async_scope(
        &mut self,
        block_id: &str,
        scope: ScopeDesc,
        ts: i64,
        span_id: i64,
        parent_span_id: i64,
    ) -> Result<bool>;
}

/// Parses async span events from a thread event block payload.
#[span_fn]
pub fn parse_async_block_payload<Proc: AsyncBlockProcessor>(
    block_id: &str,
    _object_offset: i64,
    payload: &BlockPayload,
    stream: &StreamInfo,
    processor: &mut Proc,
) -> Result<bool> {
    parse_block(stream, payload, |val| {
        if let Value::Object(obj) = val {
            match obj.type_name.as_str() {
                "BeginAsyncSpanEvent" => {
                    on_async_event(&obj, |span_desc, span_id, parent_span_id, ts| {
                        let name = span_desc.get::<Arc<String>>("name")?;
                        let filename = span_desc.get::<Arc<String>>("file")?;
                        let target = span_desc.get::<Arc<String>>("target")?;
                        let line = span_desc.get::<u32>("line")?;
                        let scope_desc = ScopeDesc::new(name, filename, target, line);
                        processor.on_begin_async_scope(
                            block_id,
                            scope_desc,
                            ts,
                            span_id as i64,
                            parent_span_id as i64,
                        )
                    })
                    .with_context(|| "reading BeginAsyncSpanEvent")
                }
                "EndAsyncSpanEvent" => {
                    on_async_event(&obj, |span_desc, span_id, parent_span_id, ts| {
                        let name = span_desc.get::<Arc<String>>("name")?;
                        let filename = span_desc.get::<Arc<String>>("file")?;
                        let target = span_desc.get::<Arc<String>>("target")?;
                        let line = span_desc.get::<u32>("line")?;
                        let scope_desc = ScopeDesc::new(name, filename, target, line);
                        processor.on_end_async_scope(
                            block_id,
                            scope_desc,
                            ts,
                            span_id as i64,
                            parent_span_id as i64,
                        )
                    })
                    .with_context(|| "reading EndAsyncSpanEvent")
                }
                "BeginAsyncNamedSpanEvent" => on_async_named_event(
                    &obj,
                    |span_location, name, span_id, parent_span_id, ts| {
                        let filename = span_location.get::<Arc<String>>("file")?;
                        let target = span_location.get::<Arc<String>>("target")?;
                        let line = span_location.get::<u32>("line")?;
                        let scope_desc = ScopeDesc::new(name, filename, target, line);
                        processor.on_begin_async_scope(
                            block_id,
                            scope_desc,
                            ts,
                            span_id as i64,
                            parent_span_id as i64,
                        )
                    },
                )
                .with_context(|| "reading BeginAsyncNamedSpanEvent"),
                "EndAsyncNamedSpanEvent" => on_async_named_event(
                    &obj,
                    |span_location, name, span_id, parent_span_id, ts| {
                        let filename = span_location.get::<Arc<String>>("file")?;
                        let target = span_location.get::<Arc<String>>("target")?;
                        let line = span_location.get::<u32>("line")?;
                        let scope_desc = ScopeDesc::new(name, filename, target, line);
                        processor.on_end_async_scope(
                            block_id,
                            scope_desc,
                            ts,
                            span_id as i64,
                            parent_span_id as i64,
                        )
                    },
                )
                .with_context(|| "reading EndAsyncNamedSpanEvent"),
                _ => Ok(true),
            }
        } else {
            Ok(true)
        }
    })
}
