//! Benchmark for the transit block parse path (`parse_block`).
//!
//! Measures per-object parse time for the representative paths:
//! - `thread_spans`: pure POD objects (`parse_pod_instance`)
//! - `measures`: POD metric objects (`parse_pod_instance`)
//! - `log_static`: string-bearing objects (interned `StaticString` dependencies)
//!
//! Run with: `cargo bench -p micromegas-analytics --bench parse_block`

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use micromegas_analytics::metadata::StreamMetadata;
use micromegas_analytics::payload::parse_block;
use micromegas_telemetry::block_wire_format::{Block, BlockPayload};
use micromegas_telemetry_sink::{stream_block::StreamBlock, stream_info::make_stream_info};
use micromegas_tracing::dispatch::make_process_info;
use micromegas_tracing::event::TracingBlock;
use micromegas_tracing::logs::{LogBlock, LogStaticStrInteropEvent, LogStream};
use micromegas_tracing::metrics::{
    IntegerMetricEvent, MetricsBlock, MetricsStream, StaticMetricMetadata,
};
use micromegas_tracing::prelude::*;
use micromegas_tracing::spans::{
    BeginThreadNamedSpanEvent, SpanLocation, ThreadBlock, ThreadStream,
};
use std::collections::HashMap;
use std::sync::Arc;

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
    let stream_info = make_stream_info(&stream);
    let meta = StreamMetadata::from_stream_info(&stream_info).unwrap();
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
    let received: Block = ciborium::from_reader(&encoded[..]).unwrap();
    let stream_info = make_stream_info(&stream);
    let meta = StreamMetadata::from_stream_info(&stream_info).unwrap();
    (meta, received.payload)
}

fn build_metric_block() -> (StreamMetadata, BlockPayload) {
    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let mut stream = MetricsStream::new(BUF, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();
    static DESC: StaticMetricMetadata = StaticMetricMetadata {
        lod: Verbosity::Med,
        name: "my_metric",
        unit: "ms",
        target: "target",
        file: "file",
        line: 123,
    };
    for i in 0..N {
        stream.get_events_mut().push(IntegerMetricEvent {
            desc: &DESC,
            value: i as u64,
            time: i as i64,
        });
    }
    let mut block =
        stream.replace_block(Arc::new(MetricsBlock::new(BUF, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info).unwrap();
    let received: Block = ciborium::from_reader(&encoded[..]).unwrap();
    let stream_info = make_stream_info(&stream);
    let meta = StreamMetadata::from_stream_info(&stream_info).unwrap();
    (meta, received.payload)
}

fn bench_parse(c: &mut Criterion) {
    let span = build_span_block();
    let metrics = build_metric_block();
    let logs = build_log_block();
    let mut group = c.benchmark_group("parse_block");
    group.throughput(Throughput::Elements(N as u64));

    group.bench_function("thread_spans", |b| {
        b.iter(|| {
            let mut count = 0u64;
            parse_block(&span.0, &span.1, |v| {
                black_box(&v);
                count += 1;
                Ok(true)
            })
            .unwrap();
            black_box(count);
        });
    });

    group.bench_function("measures", |b| {
        b.iter(|| {
            let mut count = 0u64;
            parse_block(&metrics.0, &metrics.1, |v| {
                black_box(&v);
                count += 1;
                Ok(true)
            })
            .unwrap();
            black_box(count);
        });
    });

    group.bench_function("log_static", |b| {
        b.iter(|| {
            let mut count = 0u64;
            parse_block(&logs.0, &logs.1, |v| {
                black_box(&v);
                count += 1;
                Ok(true)
            })
            .unwrap();
            black_box(count);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_parse);
criterion_main!(benches);
