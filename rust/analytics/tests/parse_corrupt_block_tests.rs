//! Corruption sweep for the transit block parse path (`read_dependencies` /
//! `parse_object_buffer`), exercised on real log / span / property-set / image
//! blocks built via the sink streams (same block-builder pattern as
//! `parse_alloc_test.rs`, `log_tests.rs`, and `image_tests.rs`).
//!
//! Unlike those tests, which drive parsing through the high-level
//! `parse_block`, this sweep decompresses the dependencies/objects buffers
//! itself and calls `read_dependencies` + `parse_object_buffer` directly —
//! mirroring what `parse_block` does internally (`payload.rs:61-76`) so each
//! sweep iteration is cheap.
//!
//! Blocks are built with a small event count (N ~ 16): the truncation sweep
//! is O(len^2), and a large buffer would be far too slow under `cargo test`.
//!
//! Two sweeps, both deterministic (no fuzzing infra, run in normal CI):
//! - Truncation: every prefix length of both buffers must parse to a
//!   `Result` (a panic fails the test).
//! - Corruption: seeded pseudo-random byte flips / large field overwrites /
//!   duplicated byte chunks must never panic (only ever `Ok` or `Err`).

use bumpalo::Bump;
use micromegas_analytics::metadata::StreamMetadata;
use micromegas_telemetry::block_wire_format::{Block, BlockPayload};
use micromegas_telemetry::compression::decompress;
use micromegas_telemetry_sink::stream_block::StreamBlock;
use micromegas_telemetry_sink::stream_info::make_stream_info;
use micromegas_tracing::dispatch::make_process_info;
use micromegas_tracing::event::TracingBlock;
use micromegas_tracing::images::{ImageBlock, ImageEvent, ImageStream};
use micromegas_tracing::levels::Level;
use micromegas_tracing::logs;
use micromegas_tracing::logs::{
    LogBlock, LogStaticStrInteropEvent, LogStream, LogStringEvent, TaggedLogString,
};
use micromegas_tracing::parsing::make_custom_readers;
use micromegas_tracing::prelude::*;
use micromegas_tracing::property_set::{Property, PropertySet};
use micromegas_tracing::spans::{
    BeginThreadNamedSpanEvent, SpanLocation, ThreadBlock, ThreadStream,
};
use micromegas_transit::{
    CustomReaderMap, DynBlob, DynString, parse_object_buffer, read_dependencies,
};
use std::collections::HashMap;
use std::sync::Arc;

/// Small on purpose: the truncation sweep below is O(len^2) per buffer.
const N: usize = 16;
const BUF: usize = 256 * 1024;

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
    let meta = StreamMetadata::from_stream_info(&make_stream_info(&stream)).unwrap();
    (meta, received_payload(encoded))
}

fn build_log_block() -> (StreamMetadata, BlockPayload) {
    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let mut stream = LogStream::new(BUF, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    static LOG_DESC: logs::LogMetadata = logs::LogMetadata {
        level: Level::Info,
        level_filter: std::sync::atomic::AtomicU32::new(logs::FILTER_LEVEL_UNSET_VALUE),
        fmt_str: "",
        target: "target_name",
        module_path: "module_path",
        file: file!(),
        line: line!(),
    };

    for i in 0..N {
        // LogStringInteropEventV2 (StringId dependency)
        stream.get_events_mut().push(LogStaticStrInteropEvent {
            time: i as i64,
            level: 2,
            target: "target_name".into(),
            msg: "my message".into(),
        });
        // LogStringEventV2 (StaticStringDependency)
        stream.get_events_mut().push(LogStringEvent {
            desc: &LOG_DESC,
            time: i as i64,
            msg: DynString(format!("dynamic message {i}")),
        });
        // TaggedLogString (property_set custom-reader dependency)
        stream.get_events_mut().push(TaggedLogString {
            desc: &LOG_DESC,
            properties: PropertySet::find_or_create(vec![
                Property::new("name", "road_width"),
                Property::new("animal", "chicken"),
            ]),
            time: i as i64,
            msg: DynString(format!("tagged message {i}")),
        });
    }

    let mut block = stream.replace_block(Arc::new(LogBlock::new(BUF, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info).unwrap();
    let meta = StreamMetadata::from_stream_info(&make_stream_info(&stream)).unwrap();
    (meta, received_payload(encoded))
}

fn build_image_block() -> (StreamMetadata, BlockPayload) {
    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let mut stream = ImageStream::new(BUF, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    let image_data = vec![0x89_u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    for i in 0..N {
        stream.get_events_mut().push(ImageEvent {
            time: i as i64,
            name: DynString(format!("heatmap_{i}")),
            format: DynString("png".to_owned()),
            data: DynBlob(image_data.clone()),
        });
    }

    let mut block = stream.replace_block(Arc::new(ImageBlock::new(BUF, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info).unwrap();
    let meta = StreamMetadata::from_stream_info(&make_stream_info(&stream)).unwrap();
    (meta, received_payload(encoded))
}

fn received_payload(encoded: Vec<u8>) -> BlockPayload {
    let received: Block = ciborium::from_reader(&encoded[..]).unwrap();
    received.payload
}

/// Mirrors what `parse_block` does internally (decompress, then
/// `read_dependencies` + `parse_object_buffer`), without going through the
/// higher-level `parse_block` wrapper, so a truncation/corruption sweep can
/// call it cheaply in a tight loop.
fn try_parse(
    meta: &StreamMetadata,
    custom_readers: &CustomReaderMap,
    deps_buf: &[u8],
    objs_buf: &[u8],
) -> anyhow::Result<()> {
    let bump = Bump::new();
    let dependencies =
        read_dependencies(&bump, custom_readers, &meta.dependencies_metadata, deps_buf)?;
    parse_object_buffer(
        &bump,
        custom_readers,
        &dependencies,
        &meta.objects_metadata,
        objs_buf,
        |_| Ok(true),
    )?;
    Ok(())
}

/// Tiny deterministic PRNG (splitmix64-style LCG) — no new dependency, and
/// stable across runs/platforms so failures are reproducible from the seed.
struct Lcg(u64);

impl Lcg {
    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next_u64() as usize) % bound
        }
    }
}

/// Applies one random corruption to a copy of `buf`: a byte flip, a
/// length/count-field-style large overwrite, or a duplicated byte chunk
/// (simulating a duplicated dependency id / repeated header).
fn corrupt_buffer(rng: &mut Lcg, buf: &[u8]) -> Vec<u8> {
    let mut out = buf.to_vec();
    if out.is_empty() {
        return out;
    }
    match rng.next_usize(3) {
        0 => {
            let flips = 1 + rng.next_usize(4);
            for _ in 0..flips {
                let idx = rng.next_usize(out.len());
                out[idx] ^= 0xFF;
            }
        }
        1 => {
            if out.len() >= 4 {
                let idx = rng.next_usize(out.len() - 3);
                out[idx..idx + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            }
        }
        _ => {
            if out.len() >= 8 {
                let src = rng.next_usize(out.len() - 7);
                let dst = rng.next_usize(out.len() - 7);
                let chunk: [u8; 8] = out[src..src + 8].try_into().unwrap();
                out[dst..dst + 8].copy_from_slice(&chunk);
            }
        }
    }
    out
}

/// Every prefix length of both buffers must parse to a `Result` — a panic
/// (bounds violation, arithmetic overflow trap, etc.) fails the test.
fn truncation_sweep(
    meta: &StreamMetadata,
    custom_readers: &CustomReaderMap,
    deps_buf: &[u8],
    objs_buf: &[u8],
) {
    for len in 0..=deps_buf.len() {
        let _ = try_parse(meta, custom_readers, &deps_buf[..len], objs_buf);
    }
    for len in 0..=objs_buf.len() {
        let _ = try_parse(meta, custom_readers, deps_buf, &objs_buf[..len]);
    }
}

/// Seeded random corruption of both buffers must never panic.
fn corruption_sweep(
    meta: &StreamMetadata,
    custom_readers: &CustomReaderMap,
    deps_buf: &[u8],
    objs_buf: &[u8],
    seed: u64,
    iterations: usize,
) {
    let mut rng = Lcg(seed);
    for _ in 0..iterations {
        let corrupt_deps = corrupt_buffer(&mut rng, deps_buf);
        let _ = try_parse(meta, custom_readers, &corrupt_deps, objs_buf);
        let corrupt_objs = corrupt_buffer(&mut rng, objs_buf);
        let _ = try_parse(meta, custom_readers, deps_buf, &corrupt_objs);
    }
}

fn sweep_block(meta: &StreamMetadata, payload: &BlockPayload, seed: u64) {
    let deps_buf = decompress(&payload.dependencies).expect("decompressing dependencies payload");
    let objs_buf = decompress(&payload.objects).expect("decompressing objects payload");
    let custom_readers = make_custom_readers();

    // Sanity check: the well-formed buffers must parse without error before
    // sweeping corruptions of them.
    try_parse(meta, &custom_readers, &deps_buf, &objs_buf).expect("valid block failed to parse");

    truncation_sweep(meta, &custom_readers, &deps_buf, &objs_buf);
    corruption_sweep(meta, &custom_readers, &deps_buf, &objs_buf, seed, 500);
}

#[test]
fn span_block_survives_truncation_and_corruption() {
    let (meta, payload) = build_span_block();
    sweep_block(&meta, &payload, 0xC0FFEE);
}

#[test]
fn log_block_survives_truncation_and_corruption() {
    let (meta, payload) = build_log_block();
    sweep_block(&meta, &payload, 0xDECAFBAD);
}

#[test]
fn image_block_survives_truncation_and_corruption() {
    let (meta, payload) = build_image_block();
    sweep_block(&meta, &payload, 0x1337);
}
