use crate::scope::ScopeDesc;
use crate::{fetch_block_payload, parse_block};
use anyhow::Result;
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_telemetry::stream_info::StreamInfo;
use micromegas_tracing::prelude::*;
use micromegas_tracing::warn;
use micromegas_transit::{Object, Value};
use std::sync::Arc;

pub trait ThreadBlockProcessor {
    fn on_begin_thread_scope(&mut self, scope: ScopeDesc, ts: i64) -> Result<()>;
    fn on_end_thread_scope(&mut self, scope: ScopeDesc, ts: i64) -> Result<()>;
}

fn on_thread_event<F>(obj: &micromegas_transit::Object, mut fun: F) -> Result<()>
where
    F: FnMut(Arc<Object>, i64) -> Result<()>,
{
    let tick = obj.get::<i64>("time")?;
    let scope = obj.get::<Arc<Object>>("thread_span_desc")?;
    fun(scope, tick)
}

fn on_thread_named_event<F>(obj: &micromegas_transit::Object, mut fun: F) -> Result<()>
where
    F: FnMut(Arc<Object>, Arc<String>, i64) -> Result<()>,
{
    let tick = obj.get::<i64>("time")?;
    let scope = obj.get::<Arc<Object>>("thread_span_location")?;
    let name = obj.get::<Arc<String>>("name")?;
    fun(scope, name, tick)
}

#[span_fn]
pub fn parse_thread_block_payload<Proc: ThreadBlockProcessor>(
    payload: &micromegas_telemetry::block_wire_format::BlockPayload,
    stream: &micromegas_telemetry::stream_info::StreamInfo,
    processor: &mut Proc,
) -> Result<()> {
    parse_block(stream, payload, |val| {
        if let Value::Object(obj) = val {
            match obj.type_name.as_str() {
                "BeginThreadSpanEvent" => {
                    if let Err(e) = on_thread_event(&obj, |scope, ts| {
                        let name = scope.get::<Arc<String>>("name")?;
                        let filename = scope.get::<Arc<String>>("file")?;
                        let line = scope.get::<u32>("line")?;
                        let scope_desc = ScopeDesc::new(name, filename, line);
                        processor.on_begin_thread_scope(scope_desc, ts)
                    }) {
                        warn!("Error reading BeginThreadSpanEvent: {:?}", e);
                    }
                }
                "EndThreadSpanEvent" => {
                    if let Err(e) = on_thread_event(&obj, |scope, ts| {
                        let name = scope.get::<Arc<String>>("name")?;
                        let filename = scope.get::<Arc<String>>("file")?;
                        let line = scope.get::<u32>("line")?;
                        let scope_desc = ScopeDesc::new(name, filename, line);
                        processor.on_end_thread_scope(scope_desc, ts)
                    }) {
                        warn!("Error reading EndThreadSpanEvent: {:?}", e);
                    }
                }
                "BeginThreadNamedSpanEvent" => {
                    if let Err(e) = on_thread_named_event(&obj, |scope, name, ts| {
                        let filename = scope.get::<Arc<String>>("file")?;
                        let line = scope.get::<u32>("line")?;
                        let scope_desc = ScopeDesc::new(name, filename, line);
                        processor.on_begin_thread_scope(scope_desc, ts)
                    }) {
                        warn!("Error reading BeginThreadNamedSpanEvent: {:?}", e);
                    }
                }
                "EndThreadNamedSpanEvent" => {
                    if let Err(e) = on_thread_named_event(&obj, |scope, name, ts| {
                        let filename = scope.get::<Arc<String>>("file")?;
                        let line = scope.get::<u32>("line")?;
                        let scope_desc = ScopeDesc::new(name, filename, line);
                        processor.on_end_thread_scope(scope_desc, ts)
                    }) {
                        warn!("Error reading EndThreadNamedSpanEvent: {:?}", e);
                    }
                }
                event_type => {
                    warn!("unknown event type {}", event_type);
                }
            }
        }
        Ok(true) //continue
    })?;
    Ok(())
}

#[span_fn]
pub async fn parse_thread_block<Proc: ThreadBlockProcessor>(
    blob_storage: Arc<BlobStorage>,
    stream: &StreamInfo,
    block_id: &str,
    processor: &mut Proc,
) -> Result<()> {
    let payload = fetch_block_payload(
        blob_storage,
        &stream.process_id,
        &stream.stream_id,
        block_id,
    )
    .await?;
    parse_thread_block_payload(&payload, stream, processor)
}
