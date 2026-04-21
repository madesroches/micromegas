use crate::metadata::StreamMetadata;
use crate::net_block_processing::{NetBlockProcessor, parse_net_block};
use crate::net_spans_table::{NetSpanRecord, NetSpanRecordBuilder};
use crate::time::ConvertTicks;
use anyhow::Result;
use micromegas_telemetry::blob_storage::BlobStorage;
use micromegas_telemetry::types::block::BlockMetadata;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

lazy_static::lazy_static! {
    static ref KIND_CONNECTION: Arc<String> = Arc::new(String::from("connection"));
    static ref KIND_OBJECT: Arc<String> = Arc::new(String::from("object"));
    static ref KIND_PROPERTY: Arc<String> = Arc::new(String::from("property"));
    static ref KIND_RPC: Arc<String> = Arc::new(String::from("rpc"));
    static ref EMPTY_NAME: Arc<String> = Arc::new(String::new());
}

/// Sentinel `parent_span_id` used for Connection roots and for any span whose
/// parent is missing. Set to `-1` so it can never collide with a real `span_id`
/// (which is the per-event `event_id`, a non-negative offset within the stream).
pub const ROOT_PARENT_SPAN_ID: i64 = -1;

#[derive(Debug)]
struct OpenSpan {
    span_id: i64,
    parent_span_id: i64,
    depth: u32,
    kind: Arc<String>,
    name: Arc<String>,
    connection_name: Arc<String>,
    is_outgoing: bool,
    begin_time_ns: i64,
    /// Cumulative bits consumed by already-closed children of this open span.
    child_bits_consumed: i64,
}

/// Builds `net_spans` rows from a stream of net events.
///
/// The builder owns an open-span stack that persists across `parse_net_block` calls;
/// callers drive multiple blocks through the same builder to stitch spans across
/// block boundaries within a contiguous block group.
pub struct NetSpanTreeBuilder<'a> {
    record_builder: &'a mut NetSpanRecordBuilder,
    stack: Vec<OpenSpan>,
    process_id: Arc<String>,
    stream_id: Arc<String>,
    convert_ticks: ConvertTicks,
}

impl<'a> NetSpanTreeBuilder<'a> {
    pub fn new(
        record_builder: &'a mut NetSpanRecordBuilder,
        process_id: Arc<String>,
        stream_id: Arc<String>,
        convert_ticks: ConvertTicks,
    ) -> Self {
        Self {
            record_builder,
            stack: Vec::new(),
            process_id,
            stream_id,
            convert_ticks,
        }
    }

    /// Returns the connection context (name + direction) inherited from the stack root.
    /// The stack root is the Connection span; descendants inherit its connection info.
    fn connection_context(&self) -> (Arc<String>, bool) {
        if let Some(root) = self.stack.first() {
            (root.connection_name.clone(), root.is_outgoing)
        } else {
            (EMPTY_NAME.clone(), false)
        }
    }

    fn parent_of_new_child(&self) -> (i64, u32, i64) {
        if let Some(top) = self.stack.last() {
            (top.span_id, top.depth + 1, top.child_bits_consumed)
        } else {
            // No parent on stack — use root sentinel and zero offsets.
            (ROOT_PARENT_SPAN_ID, 0, 0)
        }
    }

    /// Ends an open span (Connection / Object / RPC) at `event_time_ns` with `bit_size`.
    /// Pops the matching stack top and emits a row. Silently skips if the stack is empty,
    /// or if the top span's kind does not match `expected_kind` — popping a mismatched
    /// span would emit a row with the wrong `bit_size` and strand the real parent.
    fn close_span(
        &mut self,
        expected_kind: &Arc<String>,
        event_time_ns: i64,
        bit_size: i64,
    ) -> Result<bool> {
        match self.stack.last() {
            None => {
                // "End with no matching Begin" — bit attribution is unrecoverable; skip.
                debug!(
                    "net span end event with no matching begin (expected kind={})",
                    expected_kind
                );
                return Ok(true);
            }
            Some(top) if !Arc::ptr_eq(&top.kind, expected_kind) && *top.kind != **expected_kind => {
                // Stack top is a different kind than the End event expects. Popping would
                // emit a row with the wrong bit_size and leave the real parent open.
                // Skip instead — the matching End (if any) will close it correctly.
                debug!(
                    "net span stack mismatch: expected {}, got {}; skipping end event",
                    expected_kind, top.kind
                );
                return Ok(true);
            }
            Some(_) => {}
        }
        let open = self.stack.pop().expect("peeked above");
        let begin_bits = if let Some(parent) = self.stack.last() {
            parent.child_bits_consumed
        } else {
            0
        };
        let end_bits = begin_bits + bit_size;
        let connection_name = if self.stack.is_empty() {
            open.connection_name.clone()
        } else {
            self.stack[0].connection_name.clone()
        };
        let is_outgoing = if self.stack.is_empty() {
            open.is_outgoing
        } else {
            self.stack[0].is_outgoing
        };
        let record = NetSpanRecord {
            process_id: self.process_id.clone(),
            stream_id: self.stream_id.clone(),
            span_id: open.span_id,
            parent_span_id: open.parent_span_id,
            depth: open.depth,
            kind: open.kind.clone(),
            name: open.name.clone(),
            connection_name,
            is_outgoing,
            begin_bits,
            end_bits,
            bit_size,
            begin_time: open.begin_time_ns,
            end_time: event_time_ns,
        };
        self.record_builder.append(&record)?;
        if let Some(parent) = self.stack.last_mut() {
            parent.child_bits_consumed += bit_size;
        }
        Ok(true)
    }

    /// Discards any open spans without emitting synthetic rows. Bit attribution
    /// for unclosed spans is unrecoverable; this is logged at debug level.
    pub fn finish(self) {
        if !self.stack.is_empty() {
            debug!(
                "net span tree finishing with {} unclosed span(s); dropping",
                self.stack.len()
            );
        }
    }
}

impl<'a> NetBlockProcessor for NetSpanTreeBuilder<'a> {
    fn on_connection_begin(
        &mut self,
        event_id: i64,
        time: i64,
        connection_name: Arc<String>,
        is_outgoing: bool,
    ) -> Result<bool> {
        let begin_time_ns = self.convert_ticks.ticks_to_nanoseconds(time);
        self.stack.push(OpenSpan {
            span_id: event_id,
            parent_span_id: ROOT_PARENT_SPAN_ID,
            depth: 0,
            kind: KIND_CONNECTION.clone(),
            name: connection_name.clone(),
            connection_name,
            is_outgoing,
            begin_time_ns,
            child_bits_consumed: 0,
        });
        Ok(true)
    }

    fn on_connection_end(&mut self, _event_id: i64, time: i64, bit_size: i64) -> Result<bool> {
        let end_time_ns = self.convert_ticks.ticks_to_nanoseconds(time);
        self.close_span(&KIND_CONNECTION, end_time_ns, bit_size)
    }

    fn on_object_begin(
        &mut self,
        event_id: i64,
        time: i64,
        object_name: Arc<String>,
    ) -> Result<bool> {
        let begin_time_ns = self.convert_ticks.ticks_to_nanoseconds(time);
        let (parent_span_id, depth, _) = self.parent_of_new_child();
        let (connection_name, is_outgoing) = self.connection_context();
        self.stack.push(OpenSpan {
            span_id: event_id,
            parent_span_id,
            depth,
            kind: KIND_OBJECT.clone(),
            name: object_name,
            connection_name,
            is_outgoing,
            begin_time_ns,
            child_bits_consumed: 0,
        });
        Ok(true)
    }

    fn on_object_end(&mut self, _event_id: i64, time: i64, bit_size: i64) -> Result<bool> {
        let end_time_ns = self.convert_ticks.ticks_to_nanoseconds(time);
        self.close_span(&KIND_OBJECT, end_time_ns, bit_size)
    }

    fn on_property(
        &mut self,
        event_id: i64,
        time: i64,
        property_name: Arc<String>,
        bit_size: i64,
    ) -> Result<bool> {
        let event_time_ns = self.convert_ticks.ticks_to_nanoseconds(time);
        let (parent_span_id, depth, begin_bits) = self.parent_of_new_child();
        let end_bits = begin_bits + bit_size;
        let (connection_name, is_outgoing) = self.connection_context();
        let record = NetSpanRecord {
            process_id: self.process_id.clone(),
            stream_id: self.stream_id.clone(),
            span_id: event_id,
            parent_span_id,
            depth,
            kind: KIND_PROPERTY.clone(),
            name: property_name,
            connection_name,
            is_outgoing,
            begin_bits,
            end_bits,
            bit_size,
            begin_time: event_time_ns,
            end_time: event_time_ns,
        };
        self.record_builder.append(&record)?;
        if let Some(parent) = self.stack.last_mut() {
            parent.child_bits_consumed += bit_size;
        }
        Ok(true)
    }

    fn on_rpc_begin(
        &mut self,
        event_id: i64,
        time: i64,
        function_name: Arc<String>,
    ) -> Result<bool> {
        let begin_time_ns = self.convert_ticks.ticks_to_nanoseconds(time);
        let (parent_span_id, depth, _) = self.parent_of_new_child();
        let (connection_name, is_outgoing) = self.connection_context();
        self.stack.push(OpenSpan {
            span_id: event_id,
            parent_span_id,
            depth,
            kind: KIND_RPC.clone(),
            name: function_name,
            connection_name,
            is_outgoing,
            begin_time_ns,
            child_bits_consumed: 0,
        });
        Ok(true)
    }

    fn on_rpc_end(&mut self, _event_id: i64, time: i64, bit_size: i64) -> Result<bool> {
        let end_time_ns = self.convert_ticks.ticks_to_nanoseconds(time);
        self.close_span(&KIND_RPC, end_time_ns, bit_size)
    }
}

/// Drives a `NetSpanTreeBuilder` across a contiguous group of net event blocks,
/// stitching open spans across block boundaries.
#[span_fn]
pub async fn make_net_span_tree(
    blocks: &[BlockMetadata],
    record_builder: &mut NetSpanRecordBuilder,
    blob_storage: Arc<BlobStorage>,
    stream: &StreamMetadata,
    process_id: Arc<String>,
    convert_ticks: ConvertTicks,
) -> Result<()> {
    let stream_id = Arc::new(stream.stream_id.to_string());
    let mut builder = NetSpanTreeBuilder::new(record_builder, process_id, stream_id, convert_ticks);
    for block in blocks {
        parse_net_block(
            blob_storage.clone(),
            stream,
            block.block_id,
            block.object_offset,
            &mut builder,
        )
        .await?;
    }
    builder.finish();
    Ok(())
}
