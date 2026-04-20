# Net Spans View Plan

## Overview

Add a JIT view `net_spans` to the analytics lakehouse that materializes the per-process Network Tracing event stream into a tree of bandwidth-attribution spans (Connection / Object / Property / RPC). The view is parameterized by `process_id` and is intended to drive a flame-chart cell in the analytics web app where the **width of each span represents the number of bits on the wire**, not time.

The headline question this enables: *"Which actors / properties / RPCs dominate per-connection bandwidth in this process?"* — answered visually with a flame chart.

## Current State

The Unreal `MicromegasTracing` library emits net events into a stream tagged `'net'` (one stream per process). The wire format is documented in `mkdocs/docs/unreal/network-tracing.md`. Serialized member names come from `unreal/MicromegasTracing/Public/MicromegasTracing/NetMetadata.h` (snake_case, not the C++ member names):

- `NetConnectionBeginEvent { time: u64, connection_name: str, is_outgoing: u8 }`
- `NetConnectionEndEvent   { time: u64, bit_size: u32 }` — bit_size = inclusive content sum
- `NetObjectBeginEvent     { time: u64, object_name: str }`
- `NetObjectEndEvent       { time: u64, bit_size: u32 }` — bit_size = bunch/reader delta
- `NetPropertyEvent        { time: u64, property_name: str, bit_size: u32 }` — leaf, no End event
- `NetRPCBeginEvent        { time: u64, function_name: str }`
- `NetRPCEndEvent          { time: u64, bit_size: u32 }` — bit_size = parameter payload

C++ side (`unreal/MicromegasTracing/Public/MicromegasTracing/NetEvents.h` and `NetMetadata.h`) is complete and registered with the transit type system, so `parse_block(block_id)` already decodes net events into JSONB. The mkdocs Verifying Instrumentation section (§5) explicitly notes:

> A dedicated `net_events` view has **not yet been added** to `rust/analytics/src/lakehouse/`. For now, per-event inspection goes through the generic `parse_block(block_id)`. Once a first-class `net_events` view ships, these queries shorten.

There is **no** Rust-side analytics scaffolding for net events: no block processor, no view, no schema, no tests.

## Design

### Choice: per-span rows, not raw events

`async_events` emits one row per Begin/End event and defers pairing to SQL. This works for time-axis flame charts because `(begin_time, end_time)` of a span is naturally available via a self-join on `span_id`. For net spans the X-axis is **cumulative bit offset within parent**, which requires ordered tree traversal across all events in a connection — awkward in SQL, natural in the block processor. So `net_spans` materializes pre-paired spans (closer to `thread_spans` in shape, closer to `async_events` in lifecycle/keying).

### Schema — `net_spans_table_schema()`

| field | type | notes |
|---|---|---|
| `process_id` | Dictionary(Int16, Utf8) | the parameter; useful when joining downstream |
| `stream_id` | Dictionary(Int16, Utf8) | source stream (always one per process for net) |
| `block_id` | Dictionary(Int16, Utf8) | source block (for traceability) |
| `span_id` | Int64 | unique within materialization |
| `parent_span_id` | Int64 | 0 for root (Connection) |
| `depth` | UInt32 | 0 for Connection, 1+ inside |
| `kind` | Dictionary(Int16, Utf8) | `connection` / `object` / `property` / `rpc` |
| `name` | Dictionary(Int16, Utf8) | connection / object / property / function name |
| `connection_name` | Dictionary(Int16, Utf8) | enclosing connection (denormalized for fast filtering) |
| `is_outgoing` | Boolean | inherited from enclosing connection |
| `begin_bits` | Int64 | cumulative bit offset within parent (0 at connection root) |
| `end_bits` | Int64 | `begin_bits + bit_size` |
| `bit_size` | Int64 | inclusive bit size |
| `begin_time` | Timestamp(Nanosecond, +00:00) | timestamp of the span's Begin event |
| `end_time` | Timestamp(Nanosecond, +00:00) | timestamp of the span's End event; equals `begin_time` for `NetPropertyEvent` leaves (point-in-time) |

`begin_bits` / `end_bits` *and* `bit_size` are kept separately:
- the flame chart uses `begin_bits`/`end_bits` for layout (analogous to `begin`/`end` timestamps on the existing flame chart)
- aggregation queries ("top 10 properties by bandwidth") want raw `bit_size`

### Block processing algorithm

Net spans follow `thread_spans_view`'s pattern, **not** `async_events_view`'s `BlockProcessor` pattern, because we need cross-block stitching: a `NetConnectionBeginEvent` in block N must pair with the `NetConnectionEndEvent` in block N+1. The thread-spans implementation does this by sharing the `CallTreeBuilder`'s stack across all consecutive blocks in a contiguous group (see `rust/analytics/src/lakehouse/thread_spans_view.rs:138-164` — blocks are grouped where `block.begin_ticks == previous.end_ticks`, and `CallTreeBuilder` state persists across them).

Mirror that for net:

1. Define `NetSpanTreeBuilder` (analogous to `CallTreeBuilder` in `rust/analytics/src/call_tree.rs`) that implements `NetBlockProcessor`. It owns a `Vec<OpenSpan>` stack and a `NetSpanRecordBuilder`. Each `OpenSpan` carries `{span_id, parent_span_id, depth, kind, name, begin_time, child_bits_consumed, connection_name, is_outgoing}`.
2. **`NetConnectionBeginEvent`**: allocate `span_id`, push `OpenSpan { kind: connection, depth: 0, parent_span_id: 0, connection_name: name, is_outgoing, begin_time: event.time }`.
3. **`NetObjectBeginEvent`** / **`NetRPCBeginEvent`**: allocate `span_id`, parent = stack top, push with `begin_time = event.time`. Inherit `connection_name` / `is_outgoing` from the stack root.
4. **`NetPropertyEvent`** (leaf): allocate `span_id`, parent = stack top. `bit_size` from event. `begin_bits` = parent's `child_bits_consumed`. `end_bits` = `begin_bits + bit_size`. `begin_time = end_time = event.time` (point-in-time). Emit row. Then `parent.child_bits_consumed += bit_size`.
5. **End events** (`NetConnectionEndEvent` / `NetObjectEndEvent` / `NetRPCEndEvent`): pop the matching open span. `bit_size` from event. `begin_bits` = parent's `child_bits_consumed` (or 0 for root). `end_bits` = `begin_bits + bit_size`. `begin_time` from the popped open span; `end_time = event.time`. Emit row. Then `parent.child_bits_consumed += bit_size` (no-op for root).
6. The view drives the builder one block at a time across a contiguous group; state persists across `parse_net_block` calls within the group (this is what gives us cross-block stitching for free).
7. At group boundary (gap detected, or last block in partition), call `builder.finish()`. Unlike `CallTreeBuilder::finish()` (`call_tree.rs:80-98`), which only restructures the in-memory stack into a tree without touching timestamps, `NetSpanTreeBuilder::finish()` must explicitly drain the stack: for each still-open span, emit a row with `end_time = group.end_time` and `bit_size = child_bits_consumed`. This is new logic — do not copy `CallTreeBuilder::finish()` verbatim.

Both classic (peer subobjects at depth 0) and Iris (nested subobjects at depth 1+) hierarchies fall out naturally — the algorithm just follows the event order.

#### Edge handling

- **Block boundary inside a Connection scope**: handled implicitly by stack persistence across the block group. The Begin in block N and End in block N+1 stitch automatically. No special code needed.
- **Partition boundary inside a Connection scope**: `generate_process_jit_partitions_segment` (`rust/analytics/src/lakehouse/jit_partitions.rs:337-361`) splits a stream's blocks into multiple `SourceDataBlocksInMemory` when cumulative `nb_objects` exceeds `max_nb_objects` (default 20M). The `NetSpanTreeBuilder` stack only persists within one `SourceDataBlocksInMemory`, so a Connection straddling a partition split degrades to the "truly unclosed" case below on both sides. Accepted as a rare edge case — net streams are low-volume relative to the 20M-event cap, so connections crossing this boundary should be exceptional.
- **Truly unclosed spans at group end** (gap in block sequence, or the End event genuinely never came): best-effort emit per step 7 above. Silent. Connection scopes are designed to fit within block boundaries in practice, so this should be rare.
- **End with no matching Begin** (per mkdocs §4 "EndConnection events with no preceding BeginConnection"): silently skip. Unlike thread spans where `CallTreeBuilder` synthesizes a root span at `begin_range_ns`, a net End with no Begin means the bit attribution is unrecoverable — skipping is safer than emitting a row with fabricated offsets.
- **Decision-6 absorption** (the `LogMicromegasNet` warning case in mkdocs §4): from analytics' point of view this is just an extra Begin/End pair; treat each End as authoritative for its Begin.
- **Sum of children < parent bit_size**: that is the framing/overhead gap (mkdocs §2 "content-vs-wire"). Don't synthesize a filler span; aggregation queries can compute it directly (`bit_size - sum(child.bit_size)`).

### View

`NetSpansView` is a hybrid of `AsyncEventsView` (process-id keying, `'net'` stream tag discovery) and `ThreadSpansView` (manual block-grouping + `write_partition_from_rows` for cross-block stitching):

- view set name: `"net_spans"`
- parameter: `process_id` (UUID); rejects `"global"`
- schema: `net_spans_table_schema()`
- stream discovery: `generate_process_jit_partitions(..., "net")` — like `AsyncEventsView`. (At-most-one net stream per process means each partition spec contains a single stream's blocks.)
- partition write: copy the `write_partition` / `append_call_tree` / `update_partition` shape from `thread_spans_view.rs:104-198` — group consecutive blocks by `begin_ticks == previous.end_ticks`, run `NetSpanTreeBuilder` across each group with persistent stack, send each group's rows through `write_partition_from_rows`. Replace `make_call_tree` with a new `make_net_span_tree(blocks, ...)` helper that drives `NetSpanTreeBuilder` over the slice.
- time filter: matches `thread_spans_view.rs:278-291` — `begin <= query_end AND end >= query_begin` (overlap test against `begin_time` / `end_time`).
- `get_time_bounds()`: `NamedColumnsTimeBounds::new("begin_time", "end_time")`.
- `make_batch_partition_spec`: `bail!("not implemented")` — JIT only, like both reference views.

### Frontend — extend `FlameGraphCell`

The existing `FlameGraphCell` (`analytics-web-app/src/lib/screen-renderers/cells/FlameGraphCell.tsx`) hardcodes `timestampToMs(begin)` — the X-axis is always time. Generalize it to also accept Int64 / Float64 `begin`/`end` columns, with X-axis ticks formatted as `b` / `Kb` / `Mb` instead of timestamps.

Specifics:

- Detect at index-build time whether `begin`/`end` columns are Timestamp or numeric (check `arrow.schema.fields.find(f => f.name === 'begin').type`). Store an `xAxisMode: 'time' | 'bits'` on `FlameIndex`.
- Replace direct `timestampToMs(beginRaw, beginField?.type)` with a small adapter: if `xAxisMode === 'time'` use the existing helper; if `'bits'` cast to `Number(beginRaw)` directly.
- Tick formatter (around lines 494–513): branch on `xAxisMode`. New helper `formatBits(n: number): string` returning `"123 b"` / `"4.5 Kb"` / `"2.1 Mb"`.
- Tooltip (around lines 759–778): replace `formatDuration(end - begin)` with `formatBits(end - begin)` in bits mode. Add `bit_size`, `kind`, `connection_name`, `is_outgoing` to the tooltip when those columns are present (they are, for net spans; harmless to check `if (col)` for time-mode usage).
- The `lane` column already lets users group by `kind` (`SELECT ..., kind AS lane FROM view_instance('net_spans', '<pid>')`). No new lane logic needed.
- Editor placeholder (line 1103) and description: extend to mention "or a network trace by bits".

### Notebook usage (the goal)

```sql
SELECT span_id AS id,
       parent_span_id AS parent,
       name,
       depth,
       begin_bits AS begin,
       end_bits AS end,
       bit_size,
       kind,
       connection_name,
       is_outgoing
FROM view_instance('net_spans', '<process_id>')
WHERE connection_name = '127.0.0.1:7777' AND is_outgoing = false
```

With no `lane` column, all spans render in a single stack: Connection at depth 0, Objects (or RPCs) nested at depth 1, Properties as leaves at depth 2+. Width = bits. Hover shows bit_size, kind, connection, direction.

Alternative: if the user adds `kind AS lane`, they get four independent horizontal tracks (one per kind) instead of a nested stack — useful for per-kind bandwidth comparison, not for tree exploration. The two layouts are mutually exclusive; the nested-stack layout is the primary use case.

## Implementation Steps

### Phase 1 — Rust analytics core

1. **`rust/analytics/src/net_block_processing.rs`** (new) — define `NetBlockProcessor` trait, `parse_net_block_payload()`, `parse_net_block()` (the latter is the `parse_thread_block` analog that fetches payload + calls the parser). The trait has one method per event kind: `on_connection_begin`, `on_connection_end`, `on_object_begin`, `on_object_end`, `on_property`, `on_rpc_begin`, `on_rpc_end`. Field extraction uses the serialized (snake_case) names registered in `NetMetadata.h`: `obj.get::<i64>("time")`, `obj.get::<Arc<String>>("connection_name" | "object_name" | "property_name" | "function_name")`, `obj.get::<u8>("is_outgoing")`, `obj.get::<u32>("bit_size")`. The trait methods convert at the boundary: `is_outgoing: bool` = `raw_u8 != 0`, `bit_size: i64` = `raw_u32 as i64`. No Rust-side type defs needed — transit decodes from the stream's metadata.
2. **`rust/analytics/src/net_spans_table.rs`** (new) — `NetSpanRecord`, `NetSpanRecordBuilder`, `net_spans_table_schema()`. Pattern: copy `async_events_table.rs`, swap field set per the schema table above. Use `StringDictionaryBuilder<Int16Type>` for all dict columns. The builder tracks min `begin_time` and max `end_time` across all rows for `get_time_range()`.
3. **`rust/analytics/src/net_span_tree.rs`** (new) — `NetSpanTreeBuilder` (analogous to `CallTreeBuilder` in `call_tree.rs`) that implements `NetBlockProcessor`, owns the open-span stack, allocates `span_id`s monotonically across all blocks in a group, and emits rows directly into a `NetSpanRecordBuilder`. Plus `make_net_span_tree(blocks, ...)` helper analogous to `make_call_tree` — iterates a block slice and runs the builder across them with persistent state.
4. **`rust/analytics/src/lakehouse/net_spans_view.rs`** (new) — `NetSpansView` and `NetSpansViewMaker`. Compose:
   - process-id discovery (constructor pattern from `async_events_view.rs:74-89`) + `generate_process_jit_partitions(..., "net")` call pattern from `async_events_view.rs:150-161` (the stream-tag filter itself lives in `jit_partitions.rs`, keyed off the `stream_tag` parameter)
   - block grouping (consecutive `begin_ticks == prev.end_ticks`) + `write_partition_from_rows` channel pattern from `thread_spans_view.rs:104-198` — replace `append_call_tree` with `append_net_span_tree` driving `make_net_span_tree`
   - time filter / time bounds per **Design → View** above.
5. **`rust/analytics/src/lib.rs`** — add `pub mod net_block_processing;`, `pub mod net_spans_table;`, `pub mod net_span_tree;`.
6. **`rust/analytics/src/lakehouse/mod.rs`** — add `pub mod net_spans_view;`.
7. **`rust/analytics/src/lakehouse/view_factory.rs`** — register `net_spans` in the `updated_factory` block alongside `async_events` (the `async_events` registration is at ~line 295-298, *after* `updated_factory` is cloned from `factory_arc`; line 279 is where `thread_spans` is registered). Because `NetSpansView` follows the `AsyncEventsView` pattern (process-id-parameterized), `NetSpansViewMaker::new` takes an `Arc<ViewFactory>` and must be instantiated via `Arc::new(updated_factory.clone())` — same shape as the `AsyncEventsViewMaker::new(...)` call. Add a docstring block in the module-level comment at the top of the file documenting the `net_spans` schema (matches the existing `async_events` docstring block in style).

### Phase 2 — Tests

8. **`rust/analytics/tests/net_spans_test.rs`** (new) — fixture-style tests building `BlockPayload`s in-memory (pattern: reuse helpers used by existing analytics integration tests). Cover:
   - **Classic shape**: Connection → Object(A) → Property × 2 → ObjectEnd → Object(B) → ObjectEnd → ConnectionEnd. Assert depth-1 sibling structure and `begin_bits` / `end_bits` reflect cumulative-offset-within-parent correctly.
   - **Iris shape**: Connection → Object(A) → Object(child of A) → ObjectEnd → ObjectEnd → ConnectionEnd. Assert depth-2 nesting.
   - **RPC at root**: Connection → RPC → RPCEnd → ConnectionEnd.
   - **Property leaf bit accounting**: properties under an object stack their `begin_bits` correctly; final object `bit_size` may exceed sum of properties (framing gap is allowed).
   - **Cross-block stitching**: a Connection whose Begin is in block N and whose End is in block N+1 (with `block_N+1.begin_ticks == block_N.end_ticks`) produces a single row with `begin_time` from block N's Begin and `end_time` from block N+1's End — verifies that stack state persists across `parse_net_block` calls within a group.
   - **Truly unclosed at group end**: a Connection Begin with no matching End anywhere in the partition is emitted with `end_time = group.end_time` and `bit_size = child_bits_consumed`; no row leaks, no log spam.

### Phase 3 — Frontend

9. **`analytics-web-app/src/lib/screen-renderers/cells/FlameGraphCell.tsx`** — generalize per the spec in **Design → Frontend** above. Add `formatBits()` helper colocated with `formatDuration()`.
10. **`analytics-web-app/src/lib/screen-renderers/cells/__tests__/FlameGraphCell.test.tsx`** (extend existing test file if present, else create) — at minimum a unit test on `buildFlameIndex` that confirms it accepts an Arrow table whose `begin`/`end` are Int64 and produces lanes with the right `maxDepth` / row counts.

### Phase 4 — Python integration test

11. **`python/micromegas/tests/test_net_spans.py`** (new) — pattern: copy `test_async_events.py`. Find the most recent process with a `'net'`-tagged stream, query `view_instance('net_spans', '<pid>')`, assert non-empty rows, validate that for every Connection row `bit_size >= sum(direct children's bit_size)` (inclusive-size invariant).

### Phase 5 — Docs

12. **`mkdocs/docs/unreal/network-tracing.md`** §5 — replace the "view has not yet been added" paragraph with a reference to `view_instance('net_spans', '<pid>')` and shorter example queries that use it. Keep the `parse_block` example for low-level inspection.
13. **`mkdocs/docs/query-guide/`** — add a `net_spans` entry to the views/functions reference if there's a single view-listing page (verify location during implementation).

## Files to Modify

**New (Rust):**
- `rust/analytics/src/net_block_processing.rs`
- `rust/analytics/src/net_spans_table.rs`
- `rust/analytics/src/net_span_tree.rs`
- `rust/analytics/src/lakehouse/net_spans_view.rs`
- `rust/analytics/tests/net_spans_test.rs`

**Modify (Rust):**
- `rust/analytics/src/lib.rs`
- `rust/analytics/src/lakehouse/mod.rs`
- `rust/analytics/src/lakehouse/view_factory.rs`

**New (Python):**
- `python/micromegas/tests/test_net_spans.py`

**Modify (frontend):**
- `analytics-web-app/src/lib/screen-renderers/cells/FlameGraphCell.tsx`
- `analytics-web-app/src/lib/screen-renderers/cells/__tests__/FlameGraphCell.test.tsx` (if exists; else new)

**Modify (docs):**
- `mkdocs/docs/unreal/network-tracing.md`
- `mkdocs/docs/query-guide/*` (single file once located)

## Trade-offs

**Materialize spans vs. raw events.** `async_events` emits raw Begin/End events and defers pairing to SQL. We could do the same for net (call it `net_events`). Trade-off: SQL-side pairing is fine for a time axis but cumulative bit offsets need ordered tree traversal across an entire connection — possible with recursive CTEs, but slower per query and harder to read. Pre-paired spans cost a bit more in materialization but make every flame-chart query a one-liner. We pick pre-paired.

**Extend FlameGraphCell vs. new cell type.** A separate `NetFlameChartCell` would avoid touching the time-axis flame chart. Trade-off: ~95% of the rendering logic (lanes, depth packing, hit testing, WASD nav, drag-to-zoom, label clipping) is shared. Duplicating it would cost more in maintenance than the small `xAxisMode` branching in `FlameGraphCell`. We extend.

**One row per span vs. one row per Begin/End event.** One row per span doubles per-span field count (need `begin_bits` *and* `end_bits` *and* `bit_size`) but halves row count and removes the need for a self-join. Net traffic is high-frequency (per-packet, per-property), so the row-count savings matter. We choose one row per span.

**Inheriting `connection_name` / `is_outgoing` into descendants.** Could be normalized away (one Connection row carries them, descendants reference by `parent_span_id`). Trade-off: filtering "all spans on connection X" then needs a recursive CTE. Denormalizing makes filter-by-connection a flat `WHERE`. We denormalize.

## Documentation

- Update `mkdocs/docs/unreal/network-tracing.md` §5 (Verifying Instrumentation) — replace the "view has not yet been added" note with `view_instance('net_spans', '<pid>')` examples.
- Update the `view_factory.rs` doc comment block to include the `net_spans` schema (mirror the `async_events` block exactly in style).
- If `mkdocs/docs/query-guide/` has a per-view reference page or a single views table, add `net_spans`.

## Testing Strategy

End-to-end:

1. **Rust unit/integration tests**: `cd rust && cargo test -p micromegas-analytics net_spans -- --nocapture`. The fixture tests cover both classic and Iris hierarchies and the leaf-property + edge-of-block cases.
2. **CI gate**: `python3 build/rust_ci.py` from repo root (fmt, clippy, tests).
3. **Frontend lint/test**: from `analytics-web-app/`: `yarn lint && yarn test && yarn type-check`.
4. **Live integration**:
   - `python3 local_test_env/ai_scripts/start_services.py`
   - feed a real net-trace stream (run a UE client/server with the engine instrumentation from the recipe, or use a test fixture that produces net blocks)
   - `micromegas-query "SELECT name, kind, bit_size FROM view_instance('net_spans', '<pid>') ORDER BY bit_size DESC LIMIT 20"` — confirm rows return
   - assert: `SELECT bit_size FROM view_instance('net_spans', '<pid>') WHERE kind = 'connection'` ≥ `SELECT SUM(bit_size) FROM view_instance('net_spans', '<pid>') WHERE depth = 1 AND parent_span_id = <connection_span_id>` (inclusive-size invariant)
5. **Notebook visual check**: `cd analytics-web-app && yarn dev`, open a notebook, add a flame-graph cell with the SQL from **Design → Notebook usage**, confirm spans render with widths proportional to `bit_size`, X-axis ticks show `Kb`/`Mb`, tooltip shows `bit_size`/`kind`/`connection_name`.

## Out of Scope (deliberate)

- A separate raw `net_events` view (parallel to `async_events`). Useful for low-level inspection but not blocking; can be added later if usage patterns demand it.
- A combined view that joins `net_spans` with the `net.packet_*_bits` metric for content-vs-wire reconciliation. Useful, additive — not blocking.
- Python client convenience helpers in `python/micromegas/`. SQL is enough; helpers can follow once usage patterns settle.
- Materializing `bit_size` rollups at coarser grain (e.g. per-connection-per-second). The on-demand JIT view is fast enough for the headline use case; rollups can come later if dashboards need them.

## Open Questions

- **Span ID stability across materializations**: should `span_id` be stable (deterministic from event position in the block) or just unique? The async_events view treats them as opaque-but-unique, which is enough for self-joins. We adopt the same.
- **Should the frontend tooltip show parent's `connection_name` for object-kind rows even though `connection_name` is denormalized onto every row?** Probably yes — keeps the tooltip self-contained. (Implementation already covers this.)
