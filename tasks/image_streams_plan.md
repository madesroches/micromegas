# Image Streams Plan

## Overview

Add a new `images` stream type to Micromegas so instrumented applications can send screenshots or
other images as telemetry. Each image is one block on one dedicated stream, carrying a transit event
(`ImageEvent`) that holds the image name, format (e.g. `"png"`, `"jpeg"`), and raw pixel data as a
`Vec<u8>`. The analytics layer materializes a queryable `images` table with one row per image — the
row stores metadata and the raw image bytes in a `data Binary` column. The design follows the existing logs/metrics pattern end-to-end.

## Current State

### Stream type pattern

Every existing stream type follows the same structure:

1. **Event types** with `#[derive(TransitReflect)]` and `impl InProcSerialize`.
   Dynamic fields (strings, blobs) implement `InProcSize::Dynamic` and manually serialize a `u32`
   size prefix + data.
2. **Two queues** declared with `declare_queue_struct!`:
   - `*MsgQueue<EventType1, EventType2, …>` — the actual events
   - `*DepsQueue<Dependency1, …>` — string/pointer dependencies referenced by events
3. **`ExtractDeps` impl** on `MsgQueue` that walks events and deduplicates dependencies by pointer.
4. **Type aliases**: `FooBlock = EventBlock<FooMsgQueue>`, `FooStream = EventStream<FooBlock>`.

Reference: `rust/tracing/src/logs/block.rs`, `rust/tracing/src/metrics/block.rs`.

### EventSink trait

`rust/tracing/src/event/sink.rs` — one `on_init_*_stream` + `on_process_*_block` pair per stream
type. `NullEventSink` provides empty default bodies for all methods, but every new pair must be
added manually because the trait has no default implementations on the required methods.

### HttpEventSink + SinkEvent

`rust/telemetry-sink/src/http_event_sink.rs` — `SinkEvent` enum drives the background thread.
Adding a stream type means adding two variants and two `match` arms.

### StreamBlock trait

`rust/telemetry-sink/src/stream_block.rs` — generic `encode_block<Q>` function covers all existing
block types. Any new block that is an `EventBlock<Q>` gets this for free by implementing
`StreamBlock for ImageBlock { fn encode_bin … { encode_block(self, process_info) } }`.

### Stream registration

`rust/ingestion/src/web_ingestion_service.rs:insert_stream` hard-codes `FORMAT_TRANSIT` when
inserting a stream row. The generic `make_stream_info` in `rust/telemetry-sink/src/stream_info.rs`
extracts `dependencies_metadata` and `objects_metadata` from any `EventStream<Block>` that
satisfies the trait bounds — image streams qualify automatically.

### Analytics block processing

`rust/analytics/src/lakehouse/block_partition_spec.rs` — `BlockProcessorMap` maps format strings to
`Arc<dyn BlockProcessor>`. Blocks with unrecognized format strings are skipped with a warning.
Stream blocks are filtered by tag (`fetch_partition_source_data` in
`rust/analytics/src/lakehouse/partition_source_data.rs` line 261).

`rust/analytics/src/lakehouse/log_block_processor.rs` and `rust/analytics/src/log_entries_table.rs`
are the canonical reference for building a new `BlockProcessor` + Arrow schema pair.

### Dispatch

`rust/tracing/src/dispatch.rs` — `Dispatch::new()` creates streams and calls `on_init_*_stream`;
event methods push to queue and call `flush_*_buffer` when the block is full.

## Design

### New event type

```rust
// rust/tracing/src/images/image_events.rs

pub struct ImageEvent {
    pub time: i64,
    pub name: DynString,    // caller-supplied label, e.g. "screenshot", "framebuffer"
    pub format: DynString,  // "png", "jpeg", "raw-rgba", etc.
    pub data: DynBlob,      // raw bytes
}
```

`DynBlob` is a new newtype `pub struct DynBlob(pub Vec<u8>)` in `rust/transit/src/` (or inline in
the tracing crate), serialized like `DynString` but without a string codec byte — just `u32` length
followed by raw bytes. The `get_value_size` overhead for `DynBlob` is 4 bytes (no codec byte, unlike `DynString` which adds a 1-byte codec prefix).

`ImageEvent` implements `InProcSerialize` manually (same pattern as `LogStringEvent`):
- `IN_PROC_SIZE = InProcSize::Dynamic`
- `get_value_size` = `size_of::<i64>() + name.size + format.size + data.size`
- `write_value` writes time, then each dynamic field
- `read_value` reads them back in order

`ImageEvent` does **not** store pointer references to static metadata, so `ImageDepsQueue` is empty
(no dependency extraction needed).

### Queue and block types

```rust
// rust/tracing/src/images/block.rs

declare_queue_struct!(struct ImageMsgQueue<ImageEvent> {});
declare_queue_struct!(struct ImageDepsQueue<> {});   // empty — no static dependencies

impl ExtractDeps for ImageMsgQueue {
    type DepsQueue = ImageDepsQueue;
    fn extract(&self) -> Self::DepsQueue {
        ImageDepsQueue::new(0)  // nothing to extract
    }
}

pub type ImageBlock = EventBlock<ImageMsgQueue>;
pub type ImageStream = EventStream<ImageBlock>;
```

### API for instrumented apps

```rust
// rust/tracing/src/images/stream.rs  (or exposed via dispatch.rs)

pub fn send_image(name: &str, format: &str, data: Vec<u8>);
```

This is a module-level free function backed by the global `Dispatch`, mirroring `info!()` /
`imetric!()`. The stream is initialized lazily on the first call (or eagerly in `Dispatch::new()`
with tag `["image"]`).

Since images are large and infrequent, the block should be flushed **immediately** after each
`send_image` call rather than waiting for the buffer to fill. `ImageStream::hint_max_obj_size()`
can return `usize::MAX` to make `is_full()` return true after every push.

### EventSink extension

```rust
// rust/tracing/src/event/sink.rs

fn on_init_image_stream(&self, _stream: &ImageStream) {}
fn on_process_image_block(&self, _block: Arc<ImageBlock>) {}
```

Default no-op bodies mean existing `EventSink` implementors (including `NullEventSink`,
`SplitEventSink`, and Unreal's sink) need zero changes.

### HttpEventSink

Two new `SinkEvent` variants:

```
ProcessImageBlock(Arc<ImageBlock>),
```

(`InitImageStream` reuses the existing `InitStream(Arc<StreamInfo>)` variant — no change needed.)

One new match arm in `handle_sink_event` that calls the shared `push_block`.

### StreamBlock

```rust
impl StreamBlock for ImageBlock {
    fn encode_bin(&self, process_info: &ProcessInfo) -> Result<Vec<u8>> {
        encode_block(self, process_info)  // generic function already handles this
    }
}
```

### Analytics: Arrow schema

```
// rust/analytics/src/images_table.rs

images schema:
  process_id    Utf8 (dictionary)
  stream_id     Utf8 (dictionary)
  block_id      Utf8 (dictionary)
  insert_time   Timestamp(Nanosecond, UTC)
  exe           Utf8 (dictionary)
  username      Utf8 (dictionary)
  computer      Utf8 (dictionary)
  time          Timestamp(Nanosecond, UTC)   -- capture time from ImageEvent.time
  name          Utf8                         -- image label
  format        Utf8 (dictionary)            -- "png", "jpeg", etc.
  payload_size  Int64                        -- bytes of raw image data
  data          Binary                       -- raw image bytes (full pixel data)
```

Pixel data is embedded directly in the Parquet partition rows so the web app can retrieve images
via FlightSQL without direct object-store access.

### Analytics: ImageBlockProcessor

```rust
// rust/analytics/src/lakehouse/image_block_processor.rs

impl BlockProcessor for ImageBlockProcessor {
    async fn process(&self, blob_storage, src_block) -> Result<Option<PartitionRowSet>> {
        // 1. fetch_block_payload(blob_storage, process_id, stream_id, block_id)
        // 2. parse_block(src_block.stream, &payload, |value| { ... })
        //    — for each ImageEvent: append row to ImagesRecordBuilder
        // 3. fill_constant_columns for process_id, stream_id, block_id, etc.
        // 4. return PartitionRowSet with time range
    }
}
```

Registered in `images_view.rs` under `FORMAT_TRANSIT`.

### Analytics: ImagesView + ViewMaker

`rust/analytics/src/lakehouse/images_view.rs` — follows `LogView` exactly:
- `VIEW_SET_NAME = "images"`, tag filter `"image"`
- Global view instance (all processes)
- Per-process JIT view instance

Registered in `default_view_factory`:

```rust
let images_view_maker = Arc::new(ImagesViewMaker {});
updated_factory.add_global_view(images_view_maker.make_view("global")?);
updated_factory.add_view_set(String::from("images"), images_view_maker);
```

## Implementation Steps

### Phase 1 — Transit binary blob type

1. Add `DynBlob` to `rust/transit/src/` (new file `dyn_blob.rs` or inline in `lib.rs`).
   - `pub struct DynBlob(pub Vec<u8>)` with `InProcSerialize` using `u32` length prefix.
   - Re-export from `rust/transit/src/lib.rs`.

### Phase 2 — Tracing crate: images module

2. Create `rust/tracing/src/images/mod.rs`, `image_events.rs`, `block.rs`.
3. Define `ImageEvent` with `InProcSerialize` and manual `Reflect`.
4. Declare `ImageMsgQueue`, `ImageDepsQueue`, implement `ExtractDeps`.
5. Type-alias `ImageBlock` and `ImageStream`.
6. Add `pub mod images;` to `rust/tracing/src/lib.rs`.

### Phase 3 — EventSink + dispatch

7. Add `on_init_image_stream` and `on_process_image_block` to `EventSink` trait with default
   no-op bodies (`rust/tracing/src/event/sink.rs`).
8. Add `image_stream` field to `Dispatch` struct (`rust/tracing/src/dispatch.rs`).
9. In `Dispatch::new()`: create `ImageStream::new(…, &["image"], …)` and call
   `on_init_image_stream`. Use a small hardcoded buffer size (e.g. 1 MB) for
   `ImageStream` — since each `send_image` flushes immediately the buffer size
   never needs external tuning, so `init_event_dispatch`'s public signature does
   not need a new parameter.
10. Add `send_image(name, format, data)` to dispatch module and expose as free function
    in `rust/tracing/src/lib.rs` (e.g. `micromegas_tracing::send_image`).
11. Flush after every `send_image` (set `hint_max_obj_size` to `usize::MAX` on `ImageBlock`
    or flush unconditionally in `send_image`).

### Phase 4 — Telemetry sink

12. Add `ProcessImageBlock(Arc<ImageBlock>)` to `SinkEvent` enum in
    `rust/telemetry-sink/src/http_event_sink.rs`.
13. Add match arm in `handle_sink_event` calling `push_block`.
14. Implement `StreamBlock for ImageBlock` in `rust/telemetry-sink/src/stream_block.rs`.
15. Wire `on_init_image_stream` / `on_process_image_block` in `HttpEventSink`'s `EventSink` impl.

### Phase 5 — Analytics

16. Create `rust/analytics/src/images_table.rs` with `images_table_schema()` and
    `ImagesRecordBuilder`.
17. Create `rust/analytics/src/lakehouse/image_block_processor.rs` with `ImageBlockProcessor`.
18. Create `rust/analytics/src/lakehouse/images_view.rs` with `ImagesViewMaker` / `ImagesView`.
19. Register in `default_view_factory` (`rust/analytics/src/lakehouse/view_factory.rs`).
20. Expose new modules in `rust/analytics/src/lib.rs`.

### Phase 6 — Tests

21. Unit test in `rust/tracing/tests/` — call `send_image`, verify block bytes round-trip.
22. Integration test or example binary — start a local monolith, send a PNG, query
    `SELECT name, format, length(data) FROM images LIMIT 5`.

## Files to Modify

| File | Change |
|------|--------|
| `rust/transit/src/lib.rs` | Re-export `DynBlob` |
| `rust/transit/src/dyn_blob.rs` | **New** — `DynBlob` type |
| `rust/tracing/src/lib.rs` | Add `pub mod images;`, expose `send_image` |
| `rust/tracing/src/event/sink.rs` | Add default `on_init_image_stream`, `on_process_image_block` |
| `rust/tracing/src/dispatch.rs` | Add image stream field, `send_image` dispatch |
| `rust/tracing/src/images/mod.rs` | **New** |
| `rust/tracing/src/images/image_events.rs` | **New** — `ImageEvent`, `DynBlob` InProcSerialize |
| `rust/tracing/src/images/block.rs` | **New** — queues, `ImageBlock`, `ImageStream` |
| `rust/telemetry-sink/src/http_event_sink.rs` | Add `SinkEvent` variants + match arms |
| `rust/telemetry-sink/src/stream_block.rs` | `StreamBlock for ImageBlock` |
| `rust/analytics/src/lib.rs` | Expose `images_table` module |
| `rust/analytics/src/images_table.rs` | **New** — Arrow schema + record builder |
| `rust/analytics/src/lakehouse/image_block_processor.rs` | **New** — `ImageBlockProcessor` |
| `rust/analytics/src/lakehouse/images_view.rs` | **New** — `ImagesViewMaker`, `ImagesView` |
| `rust/analytics/src/lakehouse/view_factory.rs` | Register images view maker and global instance |

## Trade-offs

**Transit events vs. raw CBOR envelope**: Using a transit `ImageEvent` is strictly better than a
bespoke CBOR envelope because the schema is self-describing via `objects_metadata` — the analytics
layer uses `parse_block` generically and gets the event fields without hard-coding offsets. Schema
evolution is free: add `ImageAnnotatedEvent` later without breaking the existing decoder.

**One image per block vs. batching**: Images are large and captures are infrequent; there is no
meaningful gain from batching multiple images in one block. Flushing immediately keeps latency low,
simplifies the block processor (always exactly one row per block), and means `nb_objects = 1` is
a reliable invariant.

**Pixel data in Parquet rows**: Images are embedded directly in the `data` Binary column so the
web app can retrieve them via a normal FlightSQL query without needing direct object-store access.
Because Parquet is columnar, metadata-only queries (listing recent screenshots by name/time) skip
the `data` column entirely and remain fast regardless of image size.

**Empty `ImageDepsQueue`**: Unlike logs, `ImageEvent` has no compile-time static pointers to
extract. The `ExtractDeps` impl returns an empty queue, which is valid — `encode_block` handles
the `dependencies: compress(block.events.extract().as_bytes())` path producing an empty (but
valid) buffer.

## Documentation

- `rust/analytics/src/lakehouse/view_factory.rs` doc-comment block at the top: add `images` table
  schema table (follows the existing `log_entries`, `measures`, `thread_spans` tables).
- `mkdocs/docs/` instrumentation guide: add a section on sending images via `send_image`.

## Testing Strategy

1. **Unit**: In `rust/tracing/tests/`, use a `NullEventSink` override or a capturing sink to verify
   that `send_image("test", "png", bytes)` produces an `ImageBlock` with one event, correct name,
   format, and data.
2. **Round-trip**: In `rust/analytics/tests/` or an integration test, serialize an `ImageBlock` via
   `encode_bin`, re-parse with `parse_block`, and assert field values match.
3. **End-to-end**: Start the monolith locally, send a real PNG buffer, then run:
   ```
   micromegas-query "SELECT name, format, payload_size, length(data) FROM images LIMIT 5"
   ```
   Verify the row appears and `length(data)` matches the size of the sent buffer.

## Open Questions

- Should `send_image` be a free function in `micromegas_tracing` (like `info!`) or a method on an
  explicit `ImageStream` handle that the app keeps? A handle gives the caller control over multiple
  named streams (e.g. separate streams per camera); a free function is simpler for the common case.
