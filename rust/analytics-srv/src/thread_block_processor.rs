use std::sync::Arc;

use analytics::{fetch_block_payload, parse_block};
use anyhow::Result;
use lgn_blob_storage::BlobStorage;
use tracing::prelude::*;
use tracing::warn;
use transit::{Object, Value};

pub trait ThreadBlockProcessor {
    fn on_begin_thread_scope(
        &mut self,
        scope: Arc<Object>,
        name: Arc<String>,
        ts: i64,
    ) -> Result<()>;
    fn on_end_thread_scope(&mut self, scope: Arc<Object>, name: Arc<String>, ts: i64)
        -> Result<()>;
    fn on_begin_async_scope(
        &mut self,
        span_id: u64,
        scope: Arc<Object>,
        name: Arc<String>,
        ts: i64,
    ) -> Result<()>;
    fn on_end_async_scope(
        &mut self,
        span_id: u64,
        scope: Arc<Object>,
        name: Arc<String>,
        ts: i64,
    ) -> Result<()>;
}

fn on_thread_event<F>(obj: &transit::Object, mut fun: F) -> Result<()>
where
    F: FnMut(Arc<Object>, i64) -> Result<()>,
{
    let tick = obj.get::<i64>("time")?;
    let scope = obj.get::<Arc<Object>>("thread_span_desc")?;
    fun(scope, tick)
}

fn on_thread_named_event<F>(obj: &transit::Object, mut fun: F) -> Result<()>
where
    F: FnMut(Arc<Object>, Arc<String>, i64) -> Result<()>,
{
    let tick = obj.get::<i64>("time")?;
    let scope = obj.get::<Arc<Object>>("thread_span_location")?;
    let name = obj.get::<Arc<String>>("name")?;
    fun(scope, name, tick)
}

fn on_async_thread_event<F>(obj: &transit::Object, mut fun: F) -> Result<()>
where
    F: FnMut(u64, Arc<Object>, i64) -> Result<()>,
{
    let tick = obj.get::<i64>("time")?;
    let span_id = obj.get::<u64>("span_id")?;
    let scope = obj.get::<Arc<Object>>("span_desc")?;
    fun(span_id, scope, tick)
}

fn on_async_thread_named_event<F>(obj: &transit::Object, mut fun: F) -> Result<()>
where
    F: FnMut(u64, Arc<Object>, Arc<String>, i64) -> Result<()>,
{
    let tick = obj.get::<i64>("time")?;
    let span_id = obj.get::<u64>("span_id")?;
    let scope = obj.get::<Arc<Object>>("span_location")?;
    let name = obj.get::<Arc<String>>("name")?;
    fun(span_id, scope, name, tick)
}

#[span_fn]
pub fn parse_thread_block_payload<Proc: ThreadBlockProcessor>(
    payload: &lgn_telemetry_proto::telemetry::BlockPayload,
    stream: &telemetry_sink::StreamInfo,
    processor: &mut Proc,
) -> Result<()> {
    parse_block(stream, payload, |val| {
        if let Value::Object(obj) = val {
            match obj.type_name.as_str() {
                "BeginThreadSpanEvent" => {
                    if let Err(e) = on_thread_event(&obj, |scope, ts| {
                        let name = scope.get::<Arc<String>>("name")?;
                        processor.on_begin_thread_scope(scope, name, ts)
                    }) {
                        warn!("Error reading BeginThreadSpanEvent: {:?}", e);
                    }
                }
                "EndThreadSpanEvent" => {
                    if let Err(e) = on_thread_event(&obj, |scope, ts| {
                        let name = scope.get::<Arc<String>>("name")?;
                        processor.on_end_thread_scope(scope, name, ts)
                    }) {
                        warn!("Error reading EndThreadSpanEvent: {:?}", e);
                    }
                }
                "BeginThreadNamedSpanEvent" => {
                    if let Err(e) = on_thread_named_event(&obj, |scope, name, ts| {
                        processor.on_begin_thread_scope(scope, name, ts)
                    }) {
                        warn!("Error reading BeginThreadNamedSpanEvent: {:?}", e);
                    }
                }
                "EndThreadNamedSpanEvent" => {
                    if let Err(e) = on_thread_named_event(&obj, |scope, name, ts| {
                        processor.on_end_thread_scope(scope, name, ts)
                    }) {
                        warn!("Error reading EndThreadNamedSpanEvent: {:?}", e);
                    }
                }
                "BeginAsyncSpanEvent" => {
                    if let Err(e) = on_async_thread_event(&obj, |id, scope, ts| {
                        let name = scope.get::<Arc<String>>("name")?;
                        processor.on_begin_async_scope(id, scope, name, ts)
                    }) {
                        warn!("Error reading BeginAsyncSpanEvent: {:?}", e);
                    }
                }
                "EndAsyncSpanEvent" => {
                    if let Err(e) = on_async_thread_event(&obj, |id, scope, ts| {
                        let name = scope.get::<Arc<String>>("name")?;
                        processor.on_end_async_scope(id, scope, name, ts)
                    }) {
                        warn!("Error reading EndAsyncSpanEvent: {:?}", e);
                    }
                }
                "BeginAsyncSpanNamedEvent" => {
                    if let Err(e) = on_async_thread_named_event(&obj, |id, scope, name, ts| {
                        processor.on_begin_async_scope(id, scope, name, ts)
                    }) {
                        warn!("Error reading BeginAsyncSpanNamedEvent: {:?}", e);
                    }
                }
                "EndAsyncSpanNamedEvent" => {
                    if let Err(e) = on_async_thread_named_event(&obj, |id, scope, name, ts| {
                        processor.on_end_async_scope(id, scope, name, ts)
                    }) {
                        warn!("Error reading EndAsyncSpanNamedEvent: {:?}", e);
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
    pool: sqlx::any::AnyPool,
    blob_storage: Arc<dyn BlobStorage>,
    stream: &telemetry_sink::StreamInfo,
    block_id: String,
    processor: &mut Proc,
) -> Result<()> {
    let payload = {
        let mut connection = pool.acquire().await?;
        fetch_block_payload(&mut connection, blob_storage, block_id).await?
    };
    parse_thread_block_payload(&payload, stream, processor)
}
