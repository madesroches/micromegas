//! Wire-format tests for `BlockPayload`'s CBOR encoding: the switch to
//! `serialize_bytes` via `crate::serde_byte_buf`, and continued acceptance of
//! the legacy array-of-integers form already present in object storage.
//!
//! Item 3 (`legacy_array_form_still_decodes`) is the regression guard for
//! every block already in the lake and must never be deleted.
//!
//! Items 3 and 4 build CBOR manually via `ciborium::Value` +
//! `ciborium::into_writer`, then decode with `ciborium::from_reader` — never
//! via `Value::deserialized()` / `into_deserializer`. The `Value`-based
//! deserializer's `deserialize_byte_buf` just forwards to `deserialize_bytes`
//! (`ciborium::value::de`), which only matches `Value::Bytes` and errors on
//! `Value::Array` with no `visit_seq` fallback — unlike the real reader path
//! this module depends on. Going through `from_reader` is what actually
//! exercises the dual-path behavior under test.

use ciborium::Value;
use micromegas_telemetry::block_wire_format::BlockPayload;

/// ciborium's fixed reader scratch-buffer size (`de/mod.rs`) — the boundary
/// past which `deserialize_bytes` (as opposed to `deserialize_byte_buf`)
/// fails to decode a byte string.
const SCRATCH_SIZE: usize = 4096;

fn encode(payload: &BlockPayload) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(payload, &mut bytes).expect("encoding BlockPayload");
    bytes
}

fn decode(bytes: &[u8]) -> BlockPayload {
    ciborium::from_reader(bytes).expect("decoding BlockPayload")
}

/// Bytes spanning the full `u8` range, repeated to reach `len`.
fn pattern_bytes(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 256) as u8).collect()
}

/// Hand-builds the CBOR shape of a `BlockPayload` map from arbitrary
/// `dependencies`/`objects` values, so tests can construct either the
/// byte-string or the legacy array form directly.
fn block_payload_value(dependencies: Value, objects: Value) -> Value {
    Value::Map(vec![
        (Value::Text("dependencies".into()), dependencies),
        (Value::Text("objects".into()), objects),
    ])
}

fn encode_value(value: &Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).expect("encoding hand-built Value");
    bytes
}

fn array_of_bytes(bytes: &[u8]) -> Value {
    Value::Array(bytes.iter().map(|b| Value::from(*b)).collect())
}

// 1. New writer emits a byte string.
#[test]
fn new_writer_emits_byte_string() {
    let payload = BlockPayload {
        dependencies: vec![1, 2, 3],
        objects: pattern_bytes(300),
    };
    let bytes = encode(&payload);

    // Locate the "objects" text key (major type 3, length 7 -> 0x67) and
    // inspect the header byte of the value that immediately follows it.
    let key = b"\x67objects";
    let pos = bytes
        .windows(key.len())
        .position(|w| w == key)
        .expect("\"objects\" key not found in encoded bytes");
    let value_header = bytes[pos + key.len()];

    assert!(
        (0x40..=0x5b).contains(&value_header),
        "expected a CBOR byte-string header (major type 2), got {value_header:#04x}"
    );
    assert!(
        !(0x80..=0x9b).contains(&value_header),
        "objects field is still encoded as a CBOR array (major type 4): {value_header:#04x}"
    );
}

// 2. Round-trip across the full byte range and past the scratch-buffer size.
#[test]
fn round_trip_full_byte_range_and_large_payload() {
    let small = BlockPayload {
        dependencies: pattern_bytes(256),
        objects: pattern_bytes(256),
    };
    let decoded_small = decode(&encode(&small));
    assert_eq!(decoded_small.dependencies, small.dependencies);
    assert_eq!(decoded_small.objects, small.objects);

    // Larger than ciborium's scratch buffer: this is the case that fails if
    // the deserializer were implemented via `deserialize_bytes` instead of
    // `deserialize_byte_buf`.
    let large = BlockPayload {
        dependencies: pattern_bytes(SCRATCH_SIZE + 1000),
        objects: pattern_bytes(SCRATCH_SIZE * 3 + 7),
    };
    let decoded_large = decode(&encode(&large));
    assert_eq!(decoded_large.dependencies, large.dependencies);
    assert_eq!(decoded_large.objects, large.objects);
}

// 3. Legacy array form still decodes. Regression guard for every block
// already in storage -- do not delete.
#[test]
fn legacy_array_form_still_decodes() {
    let deps = pattern_bytes(37);
    let objs = pattern_bytes(SCRATCH_SIZE + 500);

    let value = block_payload_value(array_of_bytes(&deps), array_of_bytes(&objs));
    let bytes = encode_value(&value);

    let decoded: BlockPayload =
        ciborium::from_reader(bytes.as_slice()).expect("decoding legacy array-form CBOR");
    assert_eq!(decoded.dependencies, deps);
    assert_eq!(decoded.objects, objs);
}

// 4. New (byte-string) form decodes, at both a small size and a size above
// the scratch buffer. Covers the Unreal sink's output shape at production
// scale.
#[test]
fn byte_string_form_decodes_small_and_large() {
    let small_deps = pattern_bytes(12);
    let small_objs = pattern_bytes(64);
    let small_value = block_payload_value(
        Value::from(small_deps.clone()),
        Value::from(small_objs.clone()),
    );
    let small_bytes = encode_value(&small_value);
    let decoded_small: BlockPayload =
        ciborium::from_reader(small_bytes.as_slice()).expect("decoding small byte-string form");
    assert_eq!(decoded_small.dependencies, small_deps);
    assert_eq!(decoded_small.objects, small_objs);

    let large_deps = pattern_bytes(SCRATCH_SIZE + 1234);
    let large_objs = pattern_bytes(SCRATCH_SIZE * 2 + 17);
    let large_value = block_payload_value(
        Value::from(large_deps.clone()),
        Value::from(large_objs.clone()),
    );
    let large_bytes = encode_value(&large_value);
    let decoded_large: BlockPayload =
        ciborium::from_reader(large_bytes.as_slice()).expect("decoding large byte-string form");
    assert_eq!(decoded_large.dependencies, large_deps);
    assert_eq!(decoded_large.objects, large_objs);
}

// 5. Empty fields round-trip in both forms (the OTLP path always sends empty
// `dependencies`).
#[test]
fn empty_dependencies_round_trip_both_forms() {
    // New byte-string form: an empty `Vec<u8>` serializes to an empty byte
    // string.
    let payload = BlockPayload {
        dependencies: vec![],
        objects: pattern_bytes(64),
    };
    let decoded = decode(&encode(&payload));
    assert!(decoded.dependencies.is_empty());
    assert_eq!(decoded.objects, payload.objects);

    // Legacy array form: an empty array of integers for `dependencies`,
    // paired with a byte-string `objects` to also confirm the two fields
    // decode independently of each other's encoding.
    let objs = pattern_bytes(64);
    let value = block_payload_value(Value::Array(vec![]), Value::from(objs.clone()));
    let bytes = encode_value(&value);
    let decoded: BlockPayload = ciborium::from_reader(bytes.as_slice())
        .expect("decoding empty legacy-array-form dependencies");
    assert!(decoded.dependencies.is_empty());
    assert_eq!(decoded.objects, objs);
}

// 6. Size assertion: the byte-string form must be substantially smaller than
// the array form, for both a uniform 0..=255 payload and a payload above the
// scratch-buffer boundary.
#[test]
fn byte_string_form_is_smaller_than_legacy_array_form() {
    for len in [256usize, SCRATCH_SIZE + 1000] {
        let bytes_val = pattern_bytes(len);

        let new_form = encode(&BlockPayload {
            dependencies: vec![],
            objects: bytes_val.clone(),
        });

        let legacy_value = block_payload_value(Value::Array(vec![]), array_of_bytes(&bytes_val));
        let legacy_form = encode_value(&legacy_value);

        let ratio = new_form.len() as f64 / legacy_form.len() as f64;
        assert!(
            ratio <= 0.65,
            "byte-string form ({} bytes) is not >=35% smaller than legacy array form \
             ({} bytes) for len={len}",
            new_form.len(),
            legacy_form.len(),
        );
    }
}

// 7. Hostile size hint: a CBOR array header declaring a huge length with a
// truncated body must error out, not attempt a huge allocation or hang.
#[test]
fn hostile_size_hint_does_not_over_allocate() {
    // Hand-crafted CBOR: a 1-entry map whose "dependencies" value is an
    // array header declaring an enormous element count, immediately
    // followed by end-of-input (no actual elements). `visit_seq`'s capacity
    // is clamped to 4096 elements before any allocation happens, so this
    // must error rather than attempt to preallocate ~u64::MAX bytes.
    let mut bytes = Vec::new();
    bytes.push(0xa1); // map(1)
    bytes.push(0x6c); // text(12)
    bytes.extend_from_slice(b"dependencies");
    bytes.push(0x9b); // array, 8-byte length follows
    bytes.extend_from_slice(&u64::MAX.to_be_bytes());
    // No elements follow: the stream ends here.

    let result: Result<BlockPayload, _> = ciborium::from_reader(bytes.as_slice());
    assert!(
        result.is_err(),
        "a truncated hostile array header must fail, not hang or allocate ~u64::MAX bytes"
    );
}

/// Tiny deterministic PRNG (LCG) -- no new dependency, and stable across
/// runs/platforms so failures are reproducible from the seed. Mirrors the
/// one in `analytics/tests/parse_corrupt_block_tests.rs`.
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
/// length/count-field-style large overwrite, or a duplicated byte chunk.
fn corrupt_bytes(rng: &mut Lcg, buf: &[u8]) -> Vec<u8> {
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
                let chunk: [u8; 8] = out[src..src + 8].try_into().expect("8-byte slice");
                out[dst..dst + 8].copy_from_slice(&chunk);
            }
        }
    }
    out
}

// 8. Corrupted/truncated CBOR envelope: a truncation/corruption sweep over
// the *encoded* `BlockPayload` bytes themselves. Unlike
// `analytics/tests/parse_corrupt_block_tests.rs` (which decodes the CBOR
// envelope up front and only fuzzes the decompressed transit buffers), this
// is what actually exercises `crate::serde_byte_buf`'s deserializer against
// hostile input on the decode path.
#[test]
fn corrupted_or_truncated_envelope_never_panics() {
    let payload = BlockPayload {
        dependencies: pattern_bytes(37),
        objects: pattern_bytes(53),
    };
    let bytes = encode(&payload);

    // Sanity check: the well-formed envelope must decode before sweeping.
    let _: BlockPayload = ciborium::from_reader(bytes.as_slice()).expect("well-formed envelope");

    for len in 0..=bytes.len() {
        let _: Result<BlockPayload, _> = ciborium::from_reader(&bytes[..len]);
    }

    let mut rng = Lcg(0x5EED_1463);
    for _ in 0..500 {
        let corrupted = corrupt_bytes(&mut rng, &bytes);
        let _: Result<BlockPayload, _> = ciborium::from_reader(corrupted.as_slice());
    }
}
