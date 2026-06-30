//! Allocation-profile regression guard for the transit block parse path.
//!
//! Wraps the system allocator with a counter that is only armed around a single
//! `parse_block` call, so we can report allocations-per-object and bytes-per-object
//! for the hot parse paths. This is the headline metric for the bumpalo-arena work:
//! it should drop sharply once `Value<'a>` is arena-allocated.
//!
//! Run with: `cargo test -p micromegas-analytics --test parse_alloc_test -- --nocapture`

use micromegas_analytics::metadata::StreamMetadata;
use micromegas_analytics::payload::parse_block;
use micromegas_telemetry::block_wire_format::{Block, BlockPayload};
use micromegas_telemetry_sink::{stream_block::StreamBlock, stream_info::make_stream_info};
use micromegas_tracing::dispatch::make_process_info;
use micromegas_tracing::event::TracingBlock;
use micromegas_tracing::logs::{LogBlock, LogStaticStrInteropEvent, LogStream};
use micromegas_tracing::prelude::*;
use micromegas_tracing::spans::{
    BeginThreadNamedSpanEvent, SpanLocation, ThreadBlock, ThreadStream,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

struct Counting;

// SAFETY: forwards every call to the System allocator; the atomics only observe.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if COUNTING.load(Ordering::Relaxed) && !p.is_null() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(new_size, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const N: usize = 4096;
const BUF: usize = 16 * 1024 * 1024;

fn build_span_block() -> (StreamMetadata, BlockPayload) {
    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let mut stream = ThreadStream::new(BUF, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();
    static LOC: SpanLocation = SpanLocation {
        lod: Verbosity::Med,
        target: "target",
        module_path: "module_path",
        file: "file",
        line: 123,
    };
    for i in 0..N {
        stream.get_events_mut().push(BeginThreadNamedSpanEvent {
            thread_span_location: &LOC,
            name: "my_function".into(),
            time: i as i64,
        });
    }
    let mut block = stream.replace_block(Arc::new(ThreadBlock::new(BUF, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info).unwrap();
    let received: Block = ciborium::from_reader(&encoded[..]).unwrap();
    let meta = StreamMetadata::from_stream_info(&make_stream_info(&stream)).unwrap();
    (meta, received.payload)
}

fn build_log_block() -> (StreamMetadata, BlockPayload) {
    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let mut stream = LogStream::new(BUF, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();
    for i in 0..N {
        stream.get_events_mut().push(LogStaticStrInteropEvent {
            time: i as i64,
            level: 2,
            target: "target_name".into(),
            msg: "my log message".into(),
        });
    }
    let mut block = stream.replace_block(Arc::new(LogBlock::new(BUF, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info).unwrap();
    let meta = StreamMetadata::from_stream_info(&make_stream_info(&stream)).unwrap();
    (meta, received_payload(encoded))
}

fn received_payload(encoded: Vec<u8>) -> BlockPayload {
    let received: Block = ciborium::from_reader(&encoded[..]).unwrap();
    received.payload
}

/// Counts allocations during exactly one `parse_block` call.
fn measure(meta: &StreamMetadata, payload: &BlockPayload) -> (usize, usize, u64) {
    // Warm up once (caches, lazy statics) outside the counted region.
    parse_block(meta, payload, |_| Ok(true)).unwrap();
    ALLOCS.store(0, Ordering::SeqCst);
    BYTES.store(0, Ordering::SeqCst);
    COUNTING.store(true, Ordering::SeqCst);
    let mut count = 0u64;
    parse_block(meta, payload, |v| {
        std::hint::black_box(&v);
        count += 1;
        Ok(true)
    })
    .unwrap();
    COUNTING.store(false, Ordering::SeqCst);
    (
        ALLOCS.load(Ordering::SeqCst),
        BYTES.load(Ordering::SeqCst),
        count,
    )
}

fn report(label: &str, meta: &StreamMetadata, payload: &BlockPayload) -> f64 {
    let (allocs, bytes, count) = measure(meta, payload);
    let per_obj = allocs as f64 / count as f64;
    println!(
        "parse_block[{label}]: N={count} allocs={allocs} bytes={bytes} allocs/obj={per_obj:.3} bytes/obj={:.1}",
        bytes as f64 / count as f64
    );
    per_obj
}

#[test]
fn parse_block_allocation_profile() {
    let span = build_span_block();
    let logs = build_log_block();
    let span_per_obj = report("thread_spans", &span.0, &span.1);
    let log_per_obj = report("log_static", &logs.0, &logs.1);

    // Regression guard. With the bump arena, per-object heap allocation is
    // eliminated: only fixed per-block costs (readers map, deps map, decompress
    // buffers, arena chunks) remain, amortized to ~0.01 allocs/object at N=4096.
    // The 0.5 ceiling catches any return of per-object allocation (which would be
    // ~3-4) while leaving generous headroom for platform/allocator variance.
    // (Pre-arena baseline was ~3.0 for spans, ~4.0 for logs.)
    assert!(
        span_per_obj < 0.5,
        "thread_spans allocs/object regressed: {span_per_obj:.3} (expected ~0.01)"
    );
    assert!(
        log_per_obj < 0.5,
        "log_static allocs/object regressed: {log_per_obj:.3} (expected ~0.01)"
    );
}
