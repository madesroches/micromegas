# Harden Transit Block Parsing Against Malformed Payloads Plan

GitHub issue: https://github.com/madesroches/micromegas/issues/1192

## Overview

The transit block-parse path (`parse_block` → `read_dependencies` / `parse_object_buffer` →
custom readers) performs unchecked arithmetic, slicing, and raw-pointer reads on sizes and
offsets that come straight from the block payload. A corrupt or truncated payload can panic
the parsing thread, and in a few spots trigger undefined behavior (out-of-bounds read,
unaligned load). Block payloads come from object storage and are decoded on every
read/materialization path, so every unchecked op must be converted to a checked one that
returns `Err`.

**Acceptance (from the issue):** malformed/truncated block payloads produce an `Err` from
the parse path — never a panic and never UB.

## Current State

The untrusted parse path is:

```
analytics/src/payload.rs: parse_block()
  ├─ decompress(payload.dependencies / payload.objects)      # already fallible
  ├─ transit/src/parser.rs: read_dependencies()
  │    ├─ read_any::<u32/u64> at raw offsets                 # UB on truncated buffer
  │    ├─ StaticString: object_size - size_of::<usize>()     # underflow → panic
  │    ├─ &buffer[offset..offset + udt.size]                 # panic on OOB
  │    ├─ assert!(insert_res.is_none())                      # panic on duplicate id
  │    └─ custom readers (see below)
  ├─ transit/src/parser.rs: parse_object_buffer()
  │    ├─ read_any::<u32> at raw offset                      # UB on truncated buffer
  │    ├─ &buffer[offset..offset + object_size]              # panic on OOB
  │    └─ parse_pod_instance() / parse_custom_instance()
  └─ transit/src/parser.rs: parse_pod_instance()
       ├─ read_any::<T> at object_window.as_ptr() + member_meta.offset
       │                                                     # UB: no check offset+size ≤ window
       └─ assert_eq!(size_of::<T>(), member_meta.size)       # panic on bad metadata
```

Note: the UDT metadata (`member_meta.offset` / `.size`, `udt.size`) comes from the stream
metadata, which is also ingested from clients — it must be treated as untrusted, same as the
payload bytes.

Shared helpers (`rust/transit/src/serialize.rs`):

- `read_any<T>(ptr)` (`serialize.rs:18`) — `unsafe`, no length information at all; the
  caller must guarantee bounds. UB when the caller's offset math is wrong.
- `advance_window(window, offset)` (`serialize.rs:22`) — `assert!(offset <= window.len())`,
  panics on truncated input.
- `read_consume_pod<T>(&mut &[u8])` (`serialize.rs:27`) — panics via `advance_window`'s
  assert when the window is shorter than `size_of::<T>()` (the assert runs before the
  unaligned read, so it is a panic, not UB).

Specific spots from the issue (line numbers as of current `main`, post-#1191):

1. **`rust/transit/src/parser.rs:57-59`** — `object_size - std::mem::size_of::<usize>()`
   underflows on a bogus `object_size`; the following slice is bounds-checked (post-#1191),
   so this is now a panic rather than UB, but still not an `Err`. The `read_any::<u64>`
   at `parser.rs:56` and the `read_any::<u32>` size reads at `parser.rs:46` and
   `parser.rs:260` are genuine UB on a truncated buffer (unchecked raw-pointer reads).
2. **`rust/tracing/src/parsing.rs:289`** (`parse_image_event`) —
   `&object_window[..len as usize]` panics when the payload-supplied `len` exceeds the
   remaining window.
3. **`rust/transit/src/dyn_string.rs:76-81`** (`read_advance_string`, legacy path) — casts
   an arbitrarily aligned `&[u8]` to `*const u16` and dereferences: UB on misaligned input
   (regardless of trust — alignment of the source buffer is never guaranteed). The arena
   path `read_advance_string_in` (`dyn_string.rs:93`) already decodes via
   `chunks_exact(2)` + `u16::from_le_bytes`, but both functions slice
   `&window[0..string_len_bytes as usize]` without validating the length → panic.

Additional panic sites found while auditing (all reachable from `parse_block` with corrupt
input):

- `rust/transit/src/parser.rs:62,70,85,103` — `assert!(insert_res.is_none())` on duplicate
  dependency ids.
- `rust/tracing/src/parsing.rs` — every custom reader consumes headers with the panicking
  `read_consume_pod` and slices fixed-size sub-windows
  (`&object_window[0..string_ref_metadata.size]` at `parsing.rs:97,134,204`,
  `&window[begin..begin + property_size]` at `parsing.rs:250`) without checking the window
  is long enough.
- `rust/tracing/src/parsing.rs:241-246` (`parse_property_set`) already has the guard
  pattern we want to generalize: validate the payload-derived count against the remaining
  window and `bail!` on mismatch.

The in-proc deserialization path (`InProcSerialize::read_value`, used by the heterogeneous
queue to read events written by the *same process*, e.g. `logs/log_events.rs`,
`images/image_events.rs`, `static_string.rs:60`) shares these helpers but operates on
trusted buffers. It is **out of scope** except for the alignment UB fix in
`read_advance_string`, which is incorrect even on trusted input.

## Design

### 1. Fallible read helpers (`rust/transit/src/serialize.rs`)

Add checked counterparts next to the existing helpers (open/closed: the trusted in-proc
path keeps using the infallible ones):

```rust
/// Checked variant of `read_consume_pod`: returns Err instead of panicking
/// when the window is shorter than `size_of::<T>()`.
pub fn try_read_consume_pod<T>(window: &mut &[u8]) -> Result<T>;

/// Bounds-checked read of a POD value at `offset` within `window`.
/// Replaces `read_any(window.as_ptr().add(offset))` on untrusted windows.
pub fn try_read_pod_at<T>(window: &[u8], offset: usize) -> Result<T>;

/// Checked variant of `advance_window`: Err instead of assert.
pub fn try_advance_window<'a>(window: &'a [u8], offset: usize) -> Result<&'a [u8]>;
```

All three validate length with `checked_add` where offsets are involved, then perform the
same `std::ptr::read_unaligned` as today. Error messages should identify the failure
(`bail!("truncated window reading {}: need {need} bytes, have {have}", type_name)`), since
these errors surface in service logs when a corrupt block is encountered.

### 2. `read_dependencies` / `parse_object_buffer` (`rust/transit/src/parser.rs`)

Both loops get the same treatment:

- Dynamic-size reads (`parser.rs:46`, `parser.rs:260`): replace
  `unsafe { read_any::<u32>(buffer.as_ptr().add(offset)) }` with `try_read_pod_at::<u32>`.
- Before handing an object window to a sub-parser, validate it fits:
  `offset.checked_add(object_size)` must be `Some` and `<= buffer.len()`, else
  `bail!("corrupt block: object at offset {offset} with size {object_size} exceeds
  {}-byte buffer", ...)`. This single guard makes the subsequent
  `&buffer[offset..offset + size]` slices and the `offset += object_size` advance safe.
- `StaticString` branch: `string_id` via `try_read_pod_at::<u64>`;
  `object_size.checked_sub(std::mem::size_of::<usize>())` with `bail!` on `None`
  (the window-fits guard above already covers the slice end).
- `StaticStringDependency` branch (`parser.rs:66`): the `string_id` read consumes from a
  window covering the *entire* remaining buffer, so the window-fits guard does not
  guarantee 8 bytes remain — convert `read_consume_pod::<u64>` to
  `try_read_consume_pod::<u64>(...)?` (same treatment as the custom readers in §5).
- Replace the four `assert!(insert_res.is_none())` with
  `bail!("duplicate dependency id {id}")`.

### 3. `parse_pod_instance` (`rust/transit/src/parser.rs`)

- At the top of the member loop, validate
  `member_meta.offset.checked_add(member_meta.size)` is `Some` and
  `<= object_window.len()`; `bail!` otherwise. This covers the reference-key read, all
  intrinsic reads, and the recursive slice for nested UDTs.
- Replace the five `assert_eq!(std::mem::size_of::<T>(), member_meta.size)` with `bail!`
  (metadata is untrusted; an assert is a panic vector).
- Replace the `unsafe { read_any::<T>(...) }` calls with `try_read_pod_at::<T>` — with the
  guard above they can no longer fail, but it removes the `unsafe` blocks and keeps every
  payload read going through one audited helper.
- The nested-UDT recursion (`parser.rs:209-210`) uses `member_udt.size`, not
  `member_meta.size`; guard that pair with the same checked pattern before slicing.

### 4. String decoding (`rust/transit/src/dyn_string.rs`)

- `read_advance_string_in` (arena/untrusted path): consume codec and length via
  `try_read_consume_pod`; validate `string_len_bytes as usize <= window.len()` and `bail!`
  before slicing; advance via `try_advance_window`.
- `read_advance_string` (legacy path): same length validation, and replace the
  `cast::<u16>()` + `slice_from_raw_parts` wide decode with the alignment-safe
  `chunks_exact(2)` + `u16::from_le_bytes` + `char::decode_utf16` pattern already used in
  `read_advance_string_in` (fixes the UB). The `unsafe` block around the whole function
  body becomes unnecessary.

### 5. Custom readers (`rust/tracing/src/parsing.rs`)

All readers already return `Result`, so the changes are mechanical:

- Replace every `read_consume_pod` on `object_window` with `try_read_consume_pod(...)?`
  (with `.with_context()` naming the field, matching the existing style).
- Before the fixed-size sub-window slices (`&object_window[0..string_ref_metadata.size]`
  in the three interop readers, `&window[begin..begin + property_size]` in
  `parse_property_set`), validate the window length and `bail!` — or slice via
  `object_window.get(..size).context(...)?`.
- Replace `advance_window` with `try_advance_window` where the offset is payload-derived.
- `parse_image_event`: validate `len as usize <= object_window.len()` before
  `&object_window[..len as usize]`, mirroring the `parse_property_set` guard.

### 6. Log every corrupt-block occurrence (`rust/analytics/src/payload.rs`)

A corrupt block is unexpected enough to be a potential attack indicator, so every
occurrence must land in the service's own telemetry regardless of what the caller does
with the `Err`. `parse_block` is the single choke point every decode path goes through
(materialization processors, query-time processors, the `parse_block` table function),
and `StreamMetadata` carries the block's identity:

- In `parse_block`, on any `Err` from decompression, `read_dependencies`, or
  `parse_object_buffer`, emit an `error!` (via `micromegas_tracing::prelude::*`, already
  imported) including `stream.process_id`, `stream.stream_id`, and the error chain, then
  propagate the `Err` unchanged. `parse_block` does not know the block id — callers that
  do already attach it via `.with_context(|| format!("parse_block {}", block.block_id))`
  (e.g. `log_entry.rs:246`, `measure.rs:289`), so the propagated error stays precise
  while the choke-point log guarantees visibility.
- Error-path only: no logging is added to the hot success path.

### 7. Regression tests

Two layers, both deterministic (run in normal CI — no nightly/fuzzing infra required):

**Transit unit tests** (`rust/transit/tests/test_corrupt_input.rs`, new): direct tests of
the helpers and string decoders with hostile inputs — empty window, window shorter than the
declared length, odd wide-string byte count, invalid codec byte, `object_size` smaller than
the `StaticString` header. Assert `is_err()` on each. Wide (UTF-16, `StringCodec::Wide`)
decode coverage lives here, via hand-built buffers with a `Wide` codec byte (valid,
odd-length, and truncated variants) — no Rust write path emits `StringCodec::Wide`
(`DynString::write_value` and the static string ref both hard-code UTF-8; `Wide` is
produced only by non-Rust interop clients), so sink-built blocks cannot exercise it.

**Analytics corruption sweep** (`rust/analytics/tests/parse_corrupt_block_tests.rs`, new):
reuse the block-builder pattern from `rust/analytics/tests/parse_alloc_test.rs` (build real
log / span / property-set / image blocks via the sink streams, `encode_bin`, decode wire
format, `decompress` the dependencies and objects buffers), then drive
`read_dependencies` + `parse_object_buffer` directly (bypassing compression so each
iteration is cheap). Unlike `parse_alloc_test` (N = 4096 events, ~130–200 KB decompressed),
the sweep blocks must be built with a **small event count (N ≈ 8–32, just enough to
include one of each event kind)** — the truncation sweep is O(len²), and a
4096-event buffer would take far too long under `cargo test`; a few-KB buffer keeps the
sweep in the milliseconds range:

- **Truncation sweep**: for every prefix length `0..buf.len()` of both buffers, parse and
  require a `Result` (any panic fails the test).
- **Corruption sweep**: seeded deterministic corruption (simple xorshift/LCG inline — no
  new dependency): flip random bytes, overwrite length/count fields with large values,
  duplicate dependency ids. Parse must return `Ok` or `Err`, never panic.

UB coverage: the checked helpers make the bounds violations unreachable, and the wide
decode no longer does unaligned loads. Optionally run the transit unit tests under
`cargo +nightly miri test -p micromegas-transit` locally as a one-time verification (not
added to CI in this task).

**Optional follow-up (separate task):** a `cargo-fuzz` target over
`read_dependencies`/`parse_object_buffer` with arbitrary payloads, as suggested in the
issue. Not in scope here — it needs nightly and a `fuzz/` sub-crate that would not run in
the existing CI.

### 8. Performance guard

The parse path is hot (#1191 was a perf effort) and `rust/analytics/benches/parse_block.rs`
already benchmarks it. The added checks are branch-on-length compares in code that already
does bounds-checked slicing; run the bench before/after to confirm no measurable
regression, and note the numbers in the PR.

## Implementation Steps

1. **Helpers** — add `try_read_consume_pod`, `try_read_pod_at`, `try_advance_window` to
   `rust/transit/src/serialize.rs`; export them from `rust/transit/src/lib.rs`.
2. **Parser** — convert `read_dependencies`, `parse_object_buffer`, and
   `parse_pod_instance` in `rust/transit/src/parser.rs` to checked reads/slices and
   `bail!` per Design §2–3.
3. **Strings** — fix `read_advance_string` (alignment-safe wide decode + length guard) and
   `read_advance_string_in` (length guard, checked consumes) in
   `rust/transit/src/dyn_string.rs`.
4. **Custom readers** — convert all readers in `rust/tracing/src/parsing.rs` to the checked
   helpers; add the image-blob length guard per Design §5.
5. **Logging** — add the choke-point `error!` on parse failure in
   `rust/analytics/src/payload.rs` per Design §6.
6. **Transit tests** — new `rust/transit/tests/test_corrupt_input.rs` per Design §7.
7. **Analytics tests** — new `rust/analytics/tests/parse_corrupt_block_tests.rs` with the
   truncation and corruption sweeps per Design §7 (small event count, N ≈ 8–32, to keep
   the O(len²) truncation sweep fast).
8. **Verify** — `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`
   (workspace) from `rust/`; run `cargo bench --bench parse_block -p micromegas-analytics`
   before/after and compare.

## Files to Modify

- `rust/transit/src/serialize.rs` — new fallible helpers
- `rust/transit/src/lib.rs` — export new helpers
- `rust/transit/src/parser.rs` — checked reads/slices, `bail!` instead of assert
- `rust/transit/src/dyn_string.rs` — length guards, alignment-safe wide decode
- `rust/tracing/src/parsing.rs` — checked consumes and sub-window guards in custom readers
- `rust/analytics/src/payload.rs` — `error!` log on parse failure (choke point)
- `rust/transit/tests/test_corrupt_input.rs` — new
- `rust/analytics/tests/parse_corrupt_block_tests.rs` — new

## Trade-offs

- **Additive fallible helpers vs. making `read_consume_pod`/`advance_window` fallible
  everywhere.** Making the existing helpers return `Result` would ripple into every
  in-proc `InProcSerialize::read_value` implementation (which cannot return `Result` and
  reads trusted same-process buffers). Additive `try_*` variants harden the untrusted path
  without churning the trusted one. The cost is two nearby helpers with similar bodies;
  the infallible ones can be reimplemented on top of the fallible ones
  (`try_*(...).expect(...)`) to avoid duplication.
- **Guard-then-slice vs. `.get(..)`-style slicing.** A single up-front window-fits check
  per object (Design §2) is preferred over sprinkling `.get()` at each slice: it produces
  one clear error message with offsets/sizes, and keeps the hot loop's per-member work
  unchanged.
- **Deterministic sweep tests vs. cargo-fuzz.** The sweeps run in the existing CI on
  stable and deterministically cover the truncation class exhaustively (every prefix
  length). cargo-fuzz explores deeper mutations but requires nightly and new infra —
  deferred as an optional follow-up.
- **Strict `Err` vs. salvaging partially-parsed blocks.** A corrupt block is *very*
  unexpected and could be a form of attack, so any inconsistency drops the whole block
  hard with `Err` — no partial salvage, no warn-and-continue. After the window-fits
  guard, a truncated buffer whose last object header overruns yields `Err` instead of
  the current panic-or-silent-exit.

## Documentation

No mkdocs pages document the transit wire format or parse internals (only passing mentions
in `architecture/index.md`), so no documentation updates are needed. Rustdoc comments on
the new `try_*` helpers should state the trusted-vs-untrusted split (in-proc queue reads
may use the panicking variants; payload-derived data must use `try_*`).

## Testing Strategy

- `cargo test` from `rust/` — existing parse tests (`parse_alloc_test`, `log_tests`,
  `span_tests`, `metrics_test`, `image_tests`) prove valid blocks still parse.
- New transit unit tests prove each helper/decoder rejects hostile inputs with `Err`,
  including the wide (UTF-16) decode path via hand-built `StringCodec::Wide` buffers.
- New analytics sweep tests prove `read_dependencies`/`parse_object_buffer` never panic on
  truncated or corrupted real-world block buffers (log, span, property-set, image
  variants).
- One-time local `miri` run of the transit corrupt-input tests to confirm the UB class is
  closed.
- `cargo bench --bench parse_block` before/after to confirm no perf regression.

## Open Questions

1. **Naming:** `try_read_consume_pod` / `try_read_pod_at` / `try_advance_window` follow the
   std `try_*` convention; happy to rename (e.g. `read_consume_pod_checked`) if preferred.

## Resolved

- **Error strictness (decided):** corrupt blocks are very unexpected and could be a form
  of attack — any inconsistency fails the whole block hard with `Err`. No partial
  salvage, no warn-and-continue.
- **Logging (decided):** every corrupt-block occurrence is logged at `error!` level from
  the `parse_block` choke point with process/stream identity (Design §6), so incidents
  are visible in the service's own telemetry even if a caller swallows the `Err`.
