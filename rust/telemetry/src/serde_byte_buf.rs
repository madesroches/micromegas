//! Serde helper for encoding a `Vec<u8>` field as a CBOR byte string while
//! remaining tolerant of the legacy array-of-integers encoding produced by
//! serde's blanket `impl Serialize for Vec<T>`.
//!
//! # Why both paths exist
//!
//! `Vec<u8>`'s derived `Serialize` takes the generic sequence path, so
//! ciborium previously emitted one CBOR item per byte (major type 4, array)
//! instead of a single byte string (major type 2). For bytes >= 24 this costs
//! two bytes per byte of payload instead of one, inflating stored block
//! payloads by up to ~2x. `serialize` here switches to `serialize_bytes` to
//! fix that going forward.
//!
//! Blocks written before this change are permanent in object storage (they
//! are never rewritten), so `deserialize` must keep accepting the legacy
//! array form indefinitely. This is **not a temporary compatibility shim** —
//! do not delete the `visit_seq` path in a future cleanup; array-form blobs
//! remain in the lake forever (`analytics/src/replication.rs` also copies
//! payload blobs verbatim between lakes, so array-form objects can keep
//! arriving after the cutover indefinitely).
//!
//! # Why `deserialize_byte_buf`, not `deserialize_bytes`
//!
//! ciborium's `deserialize_byte_buf` routes both CBOR hints to the visitor: a
//! byte-string header goes to `visit_byte_buf`, and an array header goes to
//! `visit_seq`. `deserialize_bytes` also routes array headers to `visit_seq`,
//! but only accepts a byte-string header when its length fits in the
//! decoder's fixed scratch buffer (4096 bytes) and otherwise errors out —
//! rejecting both definite-length byte strings longer than that and
//! indefinite-length byte strings entirely. Real block payloads run into the
//! megabytes, so this module calls `deserialize_byte_buf`, never
//! `deserialize_bytes`.
//!
//! This dual-hint routing works only because CBOR is self-describing (the
//! byte-string vs. array distinction is carried in the header). Reusing this
//! module with a non-self-describing format (bincode, postcard) would need a
//! different approach.

use serde::de::{Deserializer, SeqAccess, Visitor};
use serde::ser::Serializer;
use std::fmt;

/// Serializes `v` as a CBOR byte string.
pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
    s.serialize_bytes(v)
}

/// Deserializes from either a CBOR byte string (current form) or a CBOR
/// array of integers (legacy form, permanent in object storage).
pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    d.deserialize_byte_buf(ByteBufVisitor)
}

struct ByteBufVisitor;

impl<'de> Visitor<'de> for ByteBufVisitor {
    type Value = Vec<u8>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a byte string or an array of integers")
    }

    fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
        Ok(v.to_vec())
    }

    fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<Self::Value, E> {
        Ok(v)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        // Legacy form: one CBOR item per byte. `seq.size_hint()` comes
        // straight from an attacker-controlled CBOR array-length header on
        // the public ingestion endpoint, so pre-allocating it directly would
        // let a tiny request declare a multi-gigabyte capacity. Clamp it —
        // deliberately stricter than serde's own 1 MiB `Vec<T>` guard — so
        // this is no weaker than what it replaces.
        let mut buf = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(4096));
        while let Some(byte) = seq.next_element::<u8>()? {
            buf.push(byte);
        }
        Ok(buf)
    }
}
