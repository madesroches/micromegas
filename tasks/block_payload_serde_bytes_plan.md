# BlockPayload byte-string CBOR encoding — Plan

Closes #1463.

## Overview

`BlockPayload`'s `dependencies` and `objects` fields are plain `Vec<u8>`, so serde's blanket
`impl Serialize for Vec<T>` takes the *sequence* path and ciborium emits a CBOR **array of
integers — one CBOR item per byte** instead of a single byte string. Bytes ≥ 24 cost two bytes
each, inflating every stored block payload by ~1.6–2.0x. This plan switches serialization to
`serialize_bytes` while keeping deserialization tolerant of **both** encodings, so the ~40–45%
storage reduction lands immediately without breaking the blocks already in the lake.

## Verification of the issue's claims

All claims below were checked empirically against ciborium 0.2.2 with a throwaway test in
`rust/telemetry/tests/` (since deleted). Results:

| Claim from #1463 | Verdict | Evidence |
|---|---|---|
| `Vec<u8>` encodes as a CBOR array, not a byte string | **Confirmed** | Encoding a `BlockPayload` with 256 objects bytes emits `99 01 00` (major type 4, array(256)) after the `objects` key — not `59` (byte string). |
| Inflation is ~1.6–1.9x and value-dependent | **Confirmed, slightly above the claimed range (2.01x measured)** | 256 raw bytes → 514 CBOR bytes (2.008x) for a uniform 0..=255 payload, where 232/256 bytes are ≥ 24. The ratio genuinely tracks the fraction of bytes ≥ 24. |
| Savings of roughly 38–46% | **Confirmed** | Same payload: old form 514 bytes, byte-string form 282 bytes → **45.1%** smaller. |
| `serde_bytes` alone will not decode the existing array form | **Confirmed** | A bytes-only visitor (`deserialize_byte_buf`) against array-form input fails with `invalid type: sequence, expected byte array`. This is the real hazard. |
| Readers must be changed *first*, before any writer, or existing readers cannot parse the new form | **Incorrect** | The *derived* `Vec<u8>` deserializer **already accepts CBOR byte strings** — ciborium's `deserialize_seq` transparently feeds a byte string as a sequence of `u8`. Decoding byte-string-form input with today's unmodified `BlockPayload` round-trips correctly. |

The last row is corroborated in production: **the Unreal sink has always written byte strings.**
`unreal/MicromegasTelemetrySink/Private/InsertBlockRequest.h:67-70` calls
`encoder.byte_string_value(compressedDep/compressedObj)`, and that traffic is ingested fine today.
So the "dual-path reader deployed everywhere first" step is a no-op for forward compatibility —
what actually needs care is *backward* compatibility, i.e. not losing the ability to read the
array-form blobs already in storage.

### The architectural fact that shrinks this change

`insert_block` decodes the client's CBOR `Block` and then **re-encodes the payload server-side**
before writing to object storage:

```rust
// rust/ingestion/src/web_ingestion_service.rs:149-151
let encoded_payload = encode_cbor(&block.payload)?;
let payload_size = encoded_payload.len();
```

The stored encoding is therefore chosen entirely by the *server's* serializer, independent of what
the client sent. Consequences:

- Unreal traffic is already compact on the wire but is **re-inflated into array form at rest**.
- Changing only the server's serializer fixes **100% of the storage inflation for every client**,
  with no client rollout and no coordination.
- The Rust `telemetry-sink` (client) picks up the same encoding automatically, since it serializes
  the same `BlockPayload` struct — there is no separate sink-side diff. Only the *rollout* of client
  builds is independent of the server deploy; see Deployment order.

## Current State

```rust
// rust/telemetry/src/block_wire_format.rs:8-12
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockPayload {
    pub dependencies: Vec<u8>,
    pub objects: Vec<u8>,
}
```

Writers of this struct:

| Writer | Form emitted today | Path |
|---|---|---|
| Rust `telemetry-sink` (client) | array (inflated) | `rust/telemetry-sink/src/stream_block.rs:34-37` → `encode_cbor(&block)` |
| Unreal sink (client) | **byte string** (already compact) | `unreal/MicromegasTelemetrySink/Private/InsertBlockRequest.h:67-70` |
| Ingestion server (to object store) | array (inflated) | `rust/ingestion/src/web_ingestion_service.rs:149` |
| OTLP adapter | n/a — builds `BlockPayload` in-process, encoded by the line above | `rust/otel-ingestion/src/block.rs:241` |

Readers:

| Reader | Path | Decodes `BlockPayload`? |
|---|---|---|
| Ingestion (request body) | `rust/ingestion/src/web_ingestion_service.rs:133` | Yes — derived `Deserialize` |
| Analytics block parsing | `rust/analytics/src/payload.rs:33-35` (`fetch_block_payload`) | Yes — derived `Deserialize` |
| Analytics `get_payload` UDF | `rust/analytics/src/lakehouse/get_payload_function.rs:107-124` | No — raw blob pass-through, byte-for-byte (`read_blob` appended straight to a `BinaryBuilder`) |
| Replication | `rust/analytics/src/replication.rs:161-171` | No — raw blob pass-through, byte-for-byte (reads the `BinaryArray` column, `put`s the bytes verbatim) |

Only two sites actually decode `BlockPayload`: the ingestion request-body reader and analytics block
parsing (`fetch_block_payload`). The `get_payload` UDF and replication move raw bytes without ever
running `Deserialize`, so byte-string vs. array-form is invisible to them. For the two real decode
sites, acceptance of both forms today happens by accident of ciborium's behavior; the plan makes that
acceptance **explicit and intentional** rather than leaving it as an undocumented dependency on a
third-party crate's internals.

`BlockPayload` is the only wire struct in `telemetry/`, `ingestion/`, or `transit/` with `Vec<u8>`
fields, so nothing else needs the same treatment.

## Design

### A local serde helper module, no new dependency

Do **not** use `serde_bytes`: it is bytes-only on the deserialize side and would reject every block
already in the lake (verified above). It is also not currently in the dependency tree. The two
functions needed are a dozen lines, so add them to the telemetry crate instead.

New module `rust/telemetry/src/serde_byte_buf.rs`:

```rust
/// Serializes as a CBOR byte string; deserializes from either a byte string
/// or the legacy array-of-integers form.
///
/// Blocks written before <version> encoded these fields as a CBOR array with one
/// item per byte (serde's blanket `Vec<T>` impl). Those objects are permanent in
/// object storage, so `deserialize` must keep accepting them indefinitely.
pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error>;
pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error>;
```

- `serialize` → `s.serialize_bytes(v)`.
- `deserialize` → `d.deserialize_byte_buf(ByteBufVisitor)`, where the visitor implements
  `visit_bytes`, `visit_byte_buf`, **and** `visit_seq`.

The dual path works because ciborium's `deserialize_byte_buf` (and `deserialize_bytes`) route both
CBOR hints to the visitor: a byte-string header goes to `visit_byte_buf`/`visit_bytes`, and an array
header goes to `visit_seq` (ciborium 0.2.2, `src/de/mod.rs:384-412` and `:364-382`). A visitor that
implements all three gets the dual path directly from `deserialize_byte_buf`, with no need for
`deserialize_any`. As an aside, this dual-hint routing is itself only available because CBOR is
self-describing; note this in a module comment — the helper would need to change to be reused with a
non-self-describing format (bincode, postcard).

Using `deserialize_byte_buf` also buys a performance win the current code does not have — the
byte-string path lands in `visit_byte_buf`/`visit_bytes` as one contiguous slice instead of being
pushed element-by-element through `SeqAccess`.

Applied to the struct:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockPayload {
    #[serde(with = "crate::serde_byte_buf")]
    pub dependencies: Vec<u8>,
    #[serde(with = "crate::serde_byte_buf")]
    pub objects: Vec<u8>,
}
```

No call site changes: the field type stays `Vec<u8>`.

### Capacity guard in `visit_seq`

`Vec::with_capacity(seq.size_hint().unwrap_or(0))` on the legacy path is an allocation-amplification
vector — this deserializer runs on the **public ingestion endpoint**, and a malicious client can
declare a multi-gigabyte CBOR array header while sending a few bytes. serde's own `Vec<T>` impl
guards this with a capacity cap (`cautious`, capping at `MAX_PREALLOC_BYTES` — 1 MiB worth of
elements, i.e. 1,048,576 for `u8`), but that logic lives in `serde_core/src/private/size_hint.rs`, a
private module. serde's `build.rs` re-exports it only under a patch-version-mangled module name (e.g.
`pub mod __private228 { pub use crate::private::*; }` for the pinned 1.0.228 in `rust/Cargo.lock`), so
there is no `serde::__private::size_hint` or `serde::de::size_hint` path stable across patch releases
for user code to call. Use an explicit clamp instead:
`Vec::with_capacity(seq.size_hint().unwrap_or(0).min(4096))` — deliberately stricter than serde's
1 MiB cap — so the new code is no weaker than what it replaces.

### Encoding shape before/after

```
before:  a2 6c "dependencies" 84 04 1822 184d 1818   <-- 0x84 array(4), 2 bytes per value >= 24
         67 "objects" 99 0100 00 01 02 ... 1818 ...

after:   a2 6c "dependencies" 44 04 22 4d 18         <-- 0x44 byte string(4), 1 byte per value
         67 "objects" 59 0100 00 01 02 ...
```

### Deployment order

Both compatibility directions hold, so ordering below is a preference, not a constraint:

- **Old client → new server**: array-form senders remain in the field indefinitely; the new
  `visit_seq` path handles them forever.
- **New client → old server**: an un-upgraded server decodes byte strings via the derived `Vec<u8>`
  deserializer, since ciborium's `deserialize_seq` accepts a byte string transparently —
  production-proven, because the Unreal sink (`InsertBlockRequest.h:67-70`) has always emitted byte
  strings and every deployed server version ingests that traffic today.

Because deployed readers already accept byte strings, strict ordering is not load-bearing here.
Still, ship in this order at zero cost, so the rollout does not depend on ciborium's incidental
behavior:

1. **Readers** — the two real decode sites: ingestion (request body) and analytics block parsing
   (`fetch_block_payload`, used by `flight-sql-srv` and `telemetry-maintenance-srv`). This is the
   same crate change, so "readers first" means deploying the *services* before the clients. (The
   `get_payload` UDF and replication move raw bytes and are unaffected either way.)
2. **Ingestion server writer** — same binary as step 1; the moment it ships, all newly stored blocks
   are compact. This is where essentially all of the storage win comes from.
3. **Rust `telemetry-sink` writer** — same crate change as steps 1–2, not a separate diff; it just
   ships inside client builds on a slower cadence, so old array-form senders remain in the field
   indefinitely after the deploy. That is fine: the ingestion reader handles both forever.

Old and new writers coexist permanently, and the array-form objects already in storage are read by
the `visit_seq` path forever. The dual-path reader is **not a temporary shim** — say so in the code
comment so a future cleanup pass does not delete it.

No metric for legacy-form decodes: because the path is permanent (blobs are never rewritten, and
`analytics/src/replication.rs:162-171` copies payload blobs verbatim between lakes, so array-form
objects can keep arriving after the cutover indefinitely), a counter meant to signal "the compat
path can be retired" would never have a consumer. It would also add a `micromegas-tracing`
dependency edge to `rust/telemetry/`, which today depends on nothing that pulls in tracing
(`Cargo.toml` lists only `anyhow`, `chrono`, `ciborium`, `lz4`, `micromegas-transit`, `serde`,
`uuid`) — not worth it for a metric with no consumer.

### Interaction with #1462 — a caveat, not a fix

#1463 suggests the value-dependent sizing is "the amplifier behind" the
`origin object changed size mid-fetch` bug in #1462. That is accurate as far as it goes, but the
implication is the wrong way round: with byte-string encoding, two payloads with the same raw length
encode to the **same** length, so the size mismatch that surfaced #1462 would no longer fire — the
duplicate-delivery divergence would become **silent** rather than fixed. This change should not be
described as addressing #1462, and #1462 must be fixed on its own terms (stabilizing `block_id` vs.
stored bytes in the OTLP path). Worth a line in the PR description so nobody closes #1462 off this.

**Rollout window caveat — re-delivery can change size at rest, and this plan does not fix it.**
`insert_block_typed` (`rust/ingestion/src/web_ingestion_service.rs:145-205`) `put`s the payload blob
unconditionally *before* the `INSERT INTO blocks … ON CONFLICT (block_id) DO NOTHING`. If a block_id
that was first stored *before* this change is redelivered *after* the ingestion server is deployed,
the object gets overwritten with the smaller byte-string encoding while the `blocks.payload_size` row
keeps the old (larger) array-form size — a real size divergence for exactly the block_ids that happen
to be re-delivered across the cutover. When the object cache is configured
(`ingestion/src/data_lake_connection.rs:90-119` wraps the store in `CacheClientStore`), it snapshots
the object size once via `origin.head()` (`object-cache/src/range_cache/mod.rs:258-294`) and treats a
length mismatch on a subsequent origin fetch as a hard error, `"origin object changed size mid-fetch"`
(`object-cache/src/range_cache/fetch.rs:386-401`). That cache entry does **not** self-heal:
`range_cache/mod.rs:54-63` documents the size and block caches as carrying no TTL, etag, or generation
in their keys and being never invalidated, and `RangeCache::size()` repeats this at `mod.rs:217-220`.
So the real blast radius is: every affected block_id gets repeated **read-path** failures — analytics
queries, ETL partition builds via `fetch_block_payload`, and the `get_payload` UDF — and the stale
cached size persists until capacity eviction, not until "expiry" or "re-fetch." There is also no
client retry to fall back on: ingestion itself succeeds (the blob `put` and the `blocks` row both
write without error), so no client ever sees an error to retry — the failure only surfaces later, on
the read side.

An earlier revision of this plan proposed closing this gap here by reordering `insert_block_typed` to
INSERT before `put` and skipping the `put` when the row already exists. That reorder is rejected for
this PR: it breaks the invariant that a committed `blocks` row implies its payload object exists.
Today's order (`put` → `INSERT`) means a crash or storage error between the two steps leaves only a
harmless orphan blob. Under the reorder, if the INSERT commits and the subsequent `put` then fails
(S3 error, OOM, process kill), the request errors, the client retries the same `block_id` (both
clients retry: the Rust sink via `tokio_retry2` in `rust/telemetry-sink/src/http_event_sink.rs:24-60,
342-362`; Unreal via `FHttpRetrySystem` in
`unreal/MicromegasTelemetrySink/Private/HttpEventSink.cpp:412-424`), `rows_affected() == 0` on the
retry, the `put` is skipped, and the blob is never written — a permanently unreadable block. That is
strictly worse than the bounded exposure it was meant to prevent.

So this plan ships without a fix for the rollout-window gap; the put-before-insert change (and the
invariant trade-off around it) stays in #1462's scope. The actual exposure differs by client because
of how `block_id` is derived: for the Rust sink, `block_id` is a UUID, so a rewrite only happens if a
client genuinely retries — a narrow deploy-window exposure. For OTLP, `block_id` is a content hash, so
any hash collision against a pre-cutover `block_id` replaces an existing array-form blob with a much
smaller byte-string one — a guaranteed large size divergence — and this keeps recurring until
pre-cutover blocks age out of retention, not just for the duration of the deploy window. That
asymmetry is an argument for landing #1462 reasonably soon after this change ships.

## Implementation Steps

1. Add `rust/telemetry/src/serde_byte_buf.rs` with `serialize`/`deserialize` and the dual-path
   visitor (`visit_bytes`, `visit_byte_buf`, `visit_seq` with a cautious capacity hint). Document
   why both paths exist and that the legacy path is permanent.
2. Register `pub mod serde_byte_buf;` in `rust/telemetry/src/lib.rs` (alphabetical, between
   `property` and `stream_info`).
3. Apply `#[serde(with = "crate::serde_byte_buf")]` to both `BlockPayload` fields in
   `rust/telemetry/src/block_wire_format.rs`, with a comment pointing at the wire-compat note.
4. Add `rust/telemetry/tests/block_wire_format_tests.rs` (see Testing Strategy).
5. Run the existing block-parsing suites unchanged — they encode with `ciborium::into_writer` and
   decode with `ciborium::from_reader`, so they exercise the new round-trip end to end:
   `analytics/tests/{log_tests,span_tests,metrics_test,image_tests,parse_alloc_test,parse_corrupt_block_tests}.rs`.
   Note `parse_corrupt_block_tests.rs` decodes the CBOR envelope up front and only fuzzes the
   decompressed transit buffers afterward, so it covers the round-trip but not corruption/truncation
   of the new deserializer itself — that gap is closed by item 8 in Testing Strategy.
6. Verify no Unreal change is needed — `byte_string_value` already emits the target form. Confirm by
   reading `InsertBlockRequest.h`; no code change expected.
7. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `python3 build/rust_ci.py`.
8. End-to-end check against a local stack (see Testing Strategy).

## Files to Modify

- `rust/telemetry/src/serde_byte_buf.rs` — **new**
- `rust/telemetry/src/lib.rs` — register module
- `rust/telemetry/src/block_wire_format.rs` — field attributes
- `rust/telemetry/tests/block_wire_format_tests.rs` — **new**

No changes required in `rust/telemetry-sink/`, `rust/analytics/`, `rust/ingestion/`,
`rust/otel-ingestion/`, or `unreal/` — the attributes propagate through the existing `encode_cbor` /
`ciborium::from_reader` call sites.

## Trade-offs

| Alternative | Why not |
|---|---|
| `#[serde(with = "serde_bytes")]` as the issue proposes | Verified to **reject** the legacy array form. Would make every block currently in the lake unparseable. Also adds a dependency for ~12 lines of code. |
| `serialize_with` only, leaving the derived `Deserialize` | Works today and is the smallest diff, but leaves back-compat resting on an undocumented ciborium behavior with nothing in the repo asserting it, and keeps the slow element-by-element decode on the hot path. The dual visitor costs a few more lines and makes the contract explicit and tested. |
| Change the object-storage format only (e.g. write raw LZ4 frames, drop the CBOR envelope) | Larger break, needs a real format version marker, and loses the self-describing envelope. The byte-string fix captures most of the win at a fraction of the risk. |
| Bump a wire format version and hard-cut | Unnecessary — the encoding is self-describing, so both forms are distinguishable from the major type alone. |
| Rewrite existing blobs to the compact form | Not worth it: retention ages them out, and a rewrite would change `payload_size` for rows already inserted, which is exactly the divergence #1462 is about. |

## Documentation

No user-facing docs describe the block CBOR layout, so nothing in `mkdocs/` needs updating.
(`mkdocs/docs/otlp/index.md` mentions the "transit/CBOR wire format" only in passing and stays
accurate.) The compatibility contract lives in the doc comment on `serde_byte_buf` and on the
`BlockPayload` fields.

Worth calling out in the PR description: expected drop in object-storage bytes and in
`blocks.payload_size` for new blocks, so the step change in storage dashboards is not read as data
loss.

## Testing Strategy

New unit tests in `rust/telemetry/tests/block_wire_format_tests.rs`:

1. **New writer emits a byte string** — encode a `BlockPayload`, assert the byte after the `objects`
   key has major type 2 (`0x40..=0x5b`), not major type 4.
2. **Round-trip** — encode/decode a payload containing bytes across the full `0..=255` range and
   assert equality.
3. **Legacy array form still decodes** — hand-build array-form CBOR with `ciborium::Value::Array`
   and assert it deserializes to the expected bytes. This is the regression guard for every block
   already in storage; it must never be deleted.
4. **New form decodes** — hand-build `ciborium::Value::Bytes` and assert it deserializes. Covers the
   Unreal sink's output shape.
5. **Empty fields** — `dependencies: vec![]` round-trips in both forms (the OTLP path always sends
   empty dependencies, per `otel-ingestion/src/block.rs:241`).
6. **Size assertion** — encoded length of the byte-string form is ≥ 35% smaller than the array form
   for a uniform `0..=255` payload, locking in the win.
7. **Hostile size hint** — a CBOR array header declaring a huge length with a truncated body errors
   out without a large allocation.
8. **Corrupted/truncated CBOR envelope** — a truncation/corruption sweep over the *encoded*
   `BlockPayload` bytes themselves (not the decompressed transit buffers), asserting every result is
   `Ok`/`Err` and never a panic or hang. This is the sweep that actually exercises the new
   `serde_byte_buf` deserializer against hostile input on the decode path.

Existing coverage that must stay green: the analytics block-parsing suites listed in step 5. Note
that `analytics/tests/parse_corrupt_block_tests.rs` does **not** cover the new deserializer: its
`received_payload()` decodes the CBOR envelope first via `ciborium::from_reader`, and `sweep_block()`
then calls `decompress()` and sweeps truncation/corruption only over the resulting decompressed
*transit* buffers, via `read_dependencies` / `parse_object_buffer`. The CBOR envelope — and therefore
`serde_byte_buf` — gets no coverage from that suite. Item 8 above closes that gap.

End-to-end:

```
python3 local_test_env/ai_scripts/start_services.py
# generate telemetry, then compare stored sizes against raw payload sizes
micromegas-query "SELECT block_id, nb_objects, payload_size FROM blocks ORDER BY insert_time DESC LIMIT 20"
```

Expect `payload_size` for new blocks to be close to the raw compressed byte count plus a small
constant envelope, rather than ~1.6–2x it. Also confirm blocks ingested *before* the change still
parse — query `log_entries` / `measures` over a time range spanning the restart, which forces
`fetch_block_payload` down the legacy path.

## Open Questions

None — both prior questions were settled from the codebase (see Design).
