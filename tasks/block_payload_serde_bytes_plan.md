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
| Inflation is ~1.6–1.9x and value-dependent | **Confirmed** | 260 raw bytes → 521 CBOR bytes (2.004x) for a uniform 0..=255 payload, where 232/256 bytes are ≥ 24. The ratio genuinely tracks the fraction of bytes ≥ 24. |
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

| Reader | Path |
|---|---|
| Ingestion (request body) | `rust/ingestion/src/web_ingestion_service.rs:133` |
| Analytics block parsing | `rust/analytics/src/payload.rs:33-35` (`fetch_block_payload`) |
| Analytics `get_payload` UDF | `rust/analytics/src/lakehouse/get_payload_function.rs:108` |
| Replication | `rust/analytics/src/replication.rs:166` |

All Rust readers go through the derived `Deserialize`, which currently accepts both forms by
accident of ciborium's behavior. The plan makes that acceptance **explicit and intentional** rather
than leaving it as an undocumented dependency on a third-party crate's internals.

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
- `deserialize` → `d.deserialize_any(ByteBufVisitor)`, where the visitor implements
  `visit_bytes`, `visit_byte_buf`, **and** `visit_seq`.

`deserialize_any` is what makes the dual path work, and it is sound here because CBOR is
self-describing. Note this in a module comment: the helper is not portable to a non-self-describing
format (bincode, postcard) without change.

Using `deserialize_any` also buys a performance win the current code does not have — the byte-string
path lands in `visit_byte_buf`/`visit_bytes` as one contiguous slice instead of being pushed
element-by-element through `SeqAccess`.

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
guards this with a capacity cap, but that logic (`serde::de::size_hint::cautious`) is not a stable
public API — as of serde 1.0.228 (pinned in `rust/Cargo.lock`) it lives under the doc-hidden
`serde::__private::size_hint`, not `serde::de`, so the visitor cannot call it. Use an explicit clamp
instead: `Vec::with_capacity(seq.size_hint().unwrap_or(0).min(4096))`, so the new code is no weaker
than what it replaces.

### Encoding shape before/after

```
before:  a2 6c "dependencies" 84 04 1822 184d 1818   <-- 0x84 array(4), 2 bytes per value >= 24
         67 "objects" 99 0100 00 01 02 ... 1818 ...

after:   a2 6c "dependencies" 44 04 22 4d 18         <-- 0x44 byte string(4), 1 byte per value
         67 "objects" 59 0100 00 01 02 ...
```

### Deployment order

Because deployed readers already accept byte strings, strict ordering is not load-bearing here.
Still, ship in this order at zero cost, so the rollout does not depend on ciborium's incidental
behavior:

1. **Readers** — analytics (`flight-sql-srv`, `telemetry-maintenance-srv`), ingestion, object-cache
   consumers. This is the same crate change, so "readers first" means deploying the *services*
   before the clients.
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

**Rollout window caveat — re-delivery can change size at rest.** `insert_block`
(`rust/ingestion/src/web_ingestion_service.rs:164-176`) `put`s the payload blob unconditionally
*before* the `INSERT INTO blocks … ON CONFLICT (block_id) DO NOTHING` at line 183. If a block_id
that was first stored *before* this change is redelivered *after* the ingestion server is deployed,
the object gets overwritten with the smaller byte-string encoding while the `blocks.payload_size`
row keeps the old (larger) array-form size — a real size divergence, for the duration of the deploy
window, for exactly the block_ids that happen to be re-delivered across the cutover. When the object
cache is configured (`ingestion/src/data_lake_connection.rs:90-119` wraps the store in
`CacheClientStore`), it snapshots the object size once via `origin.head()`
(`object-cache/src/range_cache/mod.rs:258-294`) and treats a length mismatch on a subsequent origin
fetch as a hard error, `"origin object changed size mid-fetch"`
(`object-cache/src/range_cache/fetch.rs:386-401`). This plan accepts that transient failure mode
rather than adding invalidation logic: it can only affect block_ids re-delivered within the narrow
window around one ingestion-server deploy, the client retry path already handles ingestion errors,
and the cache entry self-heals once it expires or is re-fetched after the overwrite. No code change
is scoped for this; call it out explicitly in the PR description alongside the #1462 caveat above.

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
6. Verify no Unreal change is needed — `byte_string_value` already emits the target form. Confirm by
   reading `InsertBlockRequest.h`; no code change expected.
7. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `python3 build/rust_ci.py`.
8. End-to-end check against a local stack (see Testing Strategy).

## Files to Modify

- `rust/telemetry/src/serde_byte_buf.rs` — **new**
- `rust/telemetry/src/lib.rs` — register module
- `rust/telemetry/src/block_wire_format.rs` — field attributes
- `rust/telemetry/tests/block_wire_format_tests.rs` — **new**

No changes required in `rust/ingestion/`, `rust/telemetry-sink/`, `rust/analytics/`,
`rust/otel-ingestion/`, or `unreal/` — the attributes propagate through the existing
`encode_cbor` / `ciborium::from_reader` call sites.

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

Existing coverage that must stay green: the analytics block-parsing suites listed in step 5, plus
`analytics/tests/parse_corrupt_block_tests.rs` (byte-sweep fuzzing over encoded blocks — it will now
sweep the byte-string encoding, which is the right target).

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
