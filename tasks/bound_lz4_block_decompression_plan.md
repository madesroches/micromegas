# Bound LZ4 Block Payload Decompression Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1815

## Overview
`micromegas_telemetry::compression::decompress` calls `read_to_end` with no cap on the output
size, and `parse_block` runs it on both LZ4 buffers of every block it reads. LZ4 compresses
zeros about 239:1, so one block near the 100 MiB ingestion body limit expands to about 24 GB
inside flight-sql-srv, the maintenance daemon or the monolith, which kills the process with an
OOM. This plan gives `decompress` a required maximum output size. `parse_block` passes a fixed
per-buffer ceiling, so an over-sized payload fails like any other corrupt block. This bounds
each block's decompression to a fixed ceiling, about 100× below today's worst case.

## Current State
- `rust/telemetry/src/compression.rs:25`: `decompress(compressed: &[u8]) -> Result<Vec<u8>>`
  builds an `lz4::Decoder` and calls `read_to_end` with no bound.
- `rust/analytics/src/payload.rs:69-74`: `parse_block` decompresses `payload.dependencies` and
  `payload.objects`, logs a failure through `log_decompress_err` (`corrupt block payload: ...`)
  and propagates the `Err`. Callers propagate it: materialization logs and skips the block
  (`lakehouse/block_partition_spec.rs`), while query-time paths fail the query.
- `rust/analytics/tests/parse_corrupt_block_tests.rs:269-270`: the only other caller. A test
  helper that mirrors `parse_block`.
- `rust/public/src/servers/ingestion.rs:182-183`: transit routes use `DefaultBodyLimit::disable()`
  plus `RequestBodyLimitLayer::new(100 * 1024 * 1024)`. Ingestion stores the CBOR block as-is and
  never decompresses the LZ4 buffers.
- Precedent: `rust/otel-ingestion/src/cloudwatch_logs.rs:56` caps a gzip record with
  `GzDecoder::new(raw).take(MAX + 1).read_to_end(..)` and then a length check
  (`MAX_DECOMPRESSED_RECORD_BYTES = 64 MiB`).
- Sizes that real instrumentation emits: the Rust sink defaults to 10 MiB logs and threads
  buffers and a 1 MiB metrics buffer (`rust/telemetry-sink/src/lib.rs:212-214`); the tracing
  docs example uses 16 MiB spans. Unreal uses 10 MiB log, metrics and thread buffers and 8 MiB
  for net (`unreal/MicromegasTelemetrySink/Private/HttpEventSink.cpp:549-552`). Buffer sizes are
  set through `TracingSystemGuard::new` / Unreal `Dispatch::Init`, and a configured size is not a
  hard cap: fullness is checked after the push, so the last object can push a block past it.
  The image buffer grows on demand to hold whatever `send_image` is given (any format and size),
  which LZ4 may not shrink, so it is bounded by the 100 MiB body limit. Dependency queues start
  at 1 MiB.

## Design
Make the maximum a required argument, so the compiler lists every call site:

```rust
pub fn decompress(compressed: &[u8], max_decompressed_bytes: usize) -> Result<Vec<u8>> {
    let mut decompressed = Vec::new();
    let decoder = lz4::Decoder::new(compressed).with_context(|| "allocating lz4 decoder")?;
    // `take` truncates silently; reading one byte past the cap makes the overflow detectable.
    let mut limited = decoder.take((max_decompressed_bytes as u64).saturating_add(1));
    limited.read_to_end(&mut decompressed).with_context(|| "reading lz4-compressed buffer")?;
    if decompressed.len() > max_decompressed_bytes {
        anyhow::bail!("decompressed lz4 buffer exceeds {max_decompressed_bytes} bytes");
    }
    let (_reader, res) = limited.into_inner().finish();
    res?;
    Ok(decompressed)
}
```

In `rust/analytics/src/payload.rs`:

```rust
/// Ceiling on each decompressed buffer (dependencies, objects) of one block. Real blocks are
/// at most a few tens of MiB (a configured buffer size can be exceeded by the last object), and
/// an incompressible image payload can't exceed the 100 MiB ingestion body limit. 128 MiB
/// leaves headroom for both (a raw 8K RGBA frame is about 126.6 MiB) while capping a
/// decompression bomb's footprint.
pub const MAX_DECOMPRESSED_BLOCK_BUFFER_BYTES: usize = 128 * 1024 * 1024;
```

`parse_block` passes this constant to both `decompress` calls. The existing
`log_decompress_err`/`with_context` chain logs and propagates the over-cap error unchanged.

The worst case per parsed block drops from about 24 GB to about 2 × 256 MiB: `read_to_end` grows
the `Vec` by amortized doubling and the lz4 decoder gives no size hint, so reading up to 128 MiB
can reserve about 256 MiB of capacity per buffer. Across the process the bound is roughly the
number of blocks parsed concurrently (limited by tokio worker threads, since `parse_block` is
synchronous) × ~512 MiB.

## Implementation Steps
1. `rust/telemetry/src/compression.rs`: add the `max_decompressed_bytes` parameter and the
   bounded read shown above. Update the doc comment.
2. `rust/analytics/src/payload.rs`: add `MAX_DECOMPRESSED_BLOCK_BUFFER_BYTES` and pass it in
   both `decompress` calls.
3. `rust/analytics/tests/parse_corrupt_block_tests.rs`: pass the constant in `sweep_block`.
4. Add the unit tests below.
5. `CHANGELOG.md`: add an Unreleased entry, with a **Minor breaking change** clause for the
   `decompress` signature.

## Files to Modify
- `rust/telemetry/src/compression.rs`
- `rust/telemetry/tests/compression_tests.rs` (new)
- `rust/analytics/src/payload.rs`
- `rust/analytics/tests/parse_corrupt_block_tests.rs`
- `CHANGELOG.md`

## Trade-offs
- **Required parameter vs. a hard-coded cap inside `decompress`.** The parameter keeps policy
  with the caller (block parsing), lets tests use a small cap instead of allocating 128 MiB, and
  makes the compiler find every caller.
- **128 MiB vs. a tighter cap such as 32 MiB.** A tighter cap would reject legitimate image
  blocks holding a large frame (`send_image` accepts any format and size), and blocks that
  overshoot buffers configured above the defaults. The goal is
  to stop the GB-scale blow-up, not to police block size, so the cap stays well above anything
  real.
- **Checking the decompressed size at ingestion.** Not done. Ingestion would have to decompress
  every block on its hot path, which costs CPU and memory there. The read side is where the
  memory blows up, and capping it there is enough to protect it. Rejecting blocks at ingestion
  could follow as separate work.

## Documentation
`CHANGELOG.md` only. No mkdocs page describes block decompression.

## Testing Strategy
Unit tests in a new `rust/telemetry/tests/compression_tests.rs` (alongside the existing
`block_wire_format_tests.rs`):
- Round trip: `decompress(&compress(data)?, data.len())` returns `data`, exactly at the cap.
- Over the cap: `compress(&vec![0u8; 4097])` with a 4096 cap returns `Err`, and the error
  message names the cap.
- Empty input with cap 0 round-trips.

The existing `parse_corrupt_block_tests.rs` tests keep checking that valid blocks still
decompress with the production constant.

## Decisions
- Accepted residual risk: concurrent bomb blocks can still add up to workers × ~512 MiB. A
  process-wide decompression budget, or a lower cap, is separate follow-up work; lowering the cap
  would risk rejecting legitimate large blocks.
