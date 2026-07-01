use crate::{metadata::StreamMetadata, payload::parse_block, scope::BorrowedScopeDesc};
use anyhow::{Context, Result};
use micromegas_telemetry::block_wire_format::BlockPayload;
use micromegas_tracing::prelude::*;
use micromegas_transit::value::{Object, Value};

/// Helper function to extract async event fields
fn on_async_event<'a, F>(obj: &Object<'a>, mut fun: F) -> Result<bool>
where
    F: FnMut(&'a Object<'a>, u64, u64, u32, i64) -> Result<bool>,
{
    let span_id = obj.get::<u64>("span_id")?;
    let parent_span_id = obj.get::<u64>("parent_span_id")?;
    let depth = obj.get::<u32>("depth")?;
    let time = obj.get::<i64>("time")?;
    let span_desc = obj.get::<&Object>("span_desc")?;
    fun(span_desc, span_id, parent_span_id, depth, time)
}

/// Helper function to extract async named event fields
fn on_async_named_event<'a, F>(obj: &Object<'a>, mut fun: F) -> Result<bool>
where
    F: FnMut(&'a Object<'a>, &'a str, u64, u64, u32, i64) -> Result<bool>,
{
    let span_id = obj.get::<u64>("span_id")?;
    let parent_span_id = obj.get::<u64>("parent_span_id")?;
    let depth = obj.get::<u32>("depth")?;
    let time = obj.get::<i64>("time")?;
    let span_location = obj.get::<&Object>("span_location")?;
    let name = obj.get::<&str>("name")?;
    fun(span_location, name, span_id, parent_span_id, depth, time)
}

/// Trait for processing async event blocks.
pub trait AsyncBlockProcessor {
    fn on_begin_async_scope(
        &mut self,
        block_id: &str,
        scope: BorrowedScopeDesc<'_>,
        ts: i64,
        span_id: i64,
        parent_span_id: i64,
        depth: u32,
    ) -> Result<bool>;
    fn on_end_async_scope(
        &mut self,
        block_id: &str,
        scope: BorrowedScopeDesc<'_>,
        ts: i64,
        span_id: i64,
        parent_span_id: i64,
        depth: u32,
    ) -> Result<bool>;
}

/// Parses async span events from a thread event block payload.
#[span_fn]
pub fn parse_async_block_payload<Proc: AsyncBlockProcessor>(
    block_id: &str,
    _object_offset: i64,
    payload: &BlockPayload,
    stream: &StreamMetadata,
    processor: &mut Proc,
) -> Result<bool> {
    parse_block(stream, payload, |val| {
        if let Value::Object(obj) = val {
            match obj.type_name {
                "BeginAsyncSpanEvent" => {
                    on_async_event(obj, |span_desc, span_id, parent_span_id, depth, ts| {
                        let name = span_desc.get::<&str>("name")?;
                        let filename = span_desc.get::<&str>("file")?;
                        let target = span_desc.get::<&str>("target")?;
                        let line = span_desc.get::<u32>("line")?;
                        let scope_desc = BorrowedScopeDesc::new(name, filename, target, line);
                        processor.on_begin_async_scope(
                            block_id,
                            scope_desc,
                            ts,
                            span_id as i64,
                            parent_span_id as i64,
                            depth,
                        )
                    })
                    .with_context(|| "reading BeginAsyncSpanEvent")
                }
                "EndAsyncSpanEvent" => {
                    on_async_event(obj, |span_desc, span_id, parent_span_id, depth, ts| {
                        let name = span_desc.get::<&str>("name")?;
                        let filename = span_desc.get::<&str>("file")?;
                        let target = span_desc.get::<&str>("target")?;
                        let line = span_desc.get::<u32>("line")?;
                        let scope_desc = BorrowedScopeDesc::new(name, filename, target, line);
                        processor.on_end_async_scope(
                            block_id,
                            scope_desc,
                            ts,
                            span_id as i64,
                            parent_span_id as i64,
                            depth,
                        )
                    })
                    .with_context(|| "reading EndAsyncSpanEvent")
                }
                "BeginAsyncNamedSpanEvent" => on_async_named_event(
                    obj,
                    |span_location, name, span_id, parent_span_id, depth, ts| {
                        let filename = span_location.get::<&str>("file")?;
                        let target = span_location.get::<&str>("target")?;
                        let line = span_location.get::<u32>("line")?;
                        let scope_desc = BorrowedScopeDesc::new(name, filename, target, line);
                        processor.on_begin_async_scope(
                            block_id,
                            scope_desc,
                            ts,
                            span_id as i64,
                            parent_span_id as i64,
                            depth,
                        )
                    },
                )
                .with_context(|| "reading BeginAsyncNamedSpanEvent"),
                "EndAsyncNamedSpanEvent" => on_async_named_event(
                    obj,
                    |span_location, name, span_id, parent_span_id, depth, ts| {
                        let filename = span_location.get::<&str>("file")?;
                        let target = span_location.get::<&str>("target")?;
                        let line = span_location.get::<u32>("line")?;
                        let scope_desc = BorrowedScopeDesc::new(name, filename, target, line);
                        processor.on_end_async_scope(
                            block_id,
                            scope_desc,
                            ts,
                            span_id as i64,
                            parent_span_id as i64,
                            depth,
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
