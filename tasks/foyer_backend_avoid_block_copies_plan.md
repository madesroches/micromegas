# FoyerBackend: Avoid Full-Block Copies Plan

## Overview
`FoyerBackend` currently stores cache values as `Vec<u8>` and copies the entire
block on every read hit and every fill. At the 1 MiB default block size this is
an avoidable full-block memcpy on each `get` and each `put`. Switching the stored
value type from `Vec<u8>` to `bytes::Bytes` lets RAM-tier hits and fills share the
underlying allocation (a cheap refcount bump) instead of copying. This matters on
the target 2-vCPU im4gn.large host, where full-block copies push cache serving to
be CPU / memory-bandwidth bound before it is SSD bound (see #1197).

This is issue #1195, a follow-up to the range-aware read cache (#1188).

## Current State

`FoyerBackend` (`rust/object-cache/src/foyer_backend.rs`) wraps a
`HybridCache<String, Vec<u8>>` and implements the `RangeCacheBackend` trait, whose
interface is already `Bytes`-based:

```rust
// rust/object-cache/src/backend.rs
async fn get(&self, key: &str) -> Option<Bytes>;
async fn put(&self, key: String, value: Bytes);
```

The two copy sites, `foyer_backend.rs:28-45`:

- **`get`** — `Bytes::from(entry.value().clone())`. `entry.value()` is `&Vec<u8>`;
  `.clone()` allocates and copies the whole block, then `Bytes::from(Vec)` wraps it
  (that wrap is zero-copy, but the preceding `Vec` clone is the full-block copy).
- **`put`** — `self.cache.insert(key, value.to_vec())`. `value` is already `Bytes`;
  `.to_vec()` copies the whole block into a fresh `Vec<u8>` just to satisfy the
  cache's value type.

`MemoryBackend` (`rust/object-cache/src/memory_backend.rs`) already stores `Bytes`
directly and has no copy problem — only `FoyerBackend` needs to change.

The only external construction site is
`rust/object-cache-srv/src/object_cache_srv.rs:79` (`FoyerBackend::new`). It does
not reference the internal value type, so the change is fully encapsulated behind
the existing constructor and `RangeCacheBackend` trait — no caller changes.

### Why `Bytes` satisfies Foyer's bounds

Foyer's disk tier requires the value to be `StorageValue`
(`foyer-common-0.14.1/src/code.rs:44`):

```rust
pub trait StorageValue: Value + 'static + Serialize + DeserializeOwned {}
// Value = Send + Sync + 'static
```

So the stored type must be `Send + Sync + 'static + Serialize + DeserializeOwned`.
`bytes::Bytes` is `Send + Sync + 'static` and implements `serde::Serialize` /
`Deserialize` **only when the `bytes` crate's `serde` feature is enabled**. That
feature is **not** currently enabled in this workspace (verified via
`cargo tree -e features`), so it must be turned on for the object-cache crate.

Foyer serializes disk entries with `bincode`
(`foyer-storage-0.14.1/src/serde.rs:119`). The `bytes` serde impl uses
`serialize_bytes`, which bincode writes as a length prefix + a single slice copy —
at least as efficient as the current `Vec<u8>` path (serde's default `Vec<u8>`
serialization is an element-by-element loop, since bincode does not special-case
it). So the disk path does not regress; the RAM path is where the copies are
eliminated.

## Design

Change the stored value type from `Vec<u8>` to `Bytes` and drop both copies.

```rust
pub struct FoyerBackend {
    cache: HybridCache<String, Bytes>,
}
```

```rust
async fn get(&self, key: &str) -> Option<Bytes> {
    match self.cache.obtain(key.to_string()).await {
        Ok(Some(entry)) => Some(entry.value().clone()), // Bytes clone = refcount bump
        Ok(None) => None,
        Err(e) => {
            imetric!("range_cache_backend_error", "count", 1_u64);
            warn!("range_cache backend get error key={key}: {e}");
            None
        }
    }
}

async fn put(&self, key: String, value: Bytes) {
    self.cache.insert(key, value); // no to_vec()
}
```

The error-handling branch in `get` is unchanged.

Enable the `bytes` serde feature for the crate (`rust/object-cache/Cargo.toml`):

```toml
bytes = { workspace = true, features = ["serde"] }
```

(Cargo features are additive across the workspace, so this is harmless to other
crates that depend on `bytes`.)

### Memory-retention note (no action, but worth recording)

Storing `Bytes` shares the underlying buffer. If a `Bytes` handed to `put` were a
small *sub-slice* of a much larger buffer, retaining it in the cache would pin the
whole parent allocation. In practice this is not a concern: the value passed to
`put` on the fill path is the result of `origin.get_range(...)`
(`range_cache.rs:137,146`), which returns a freshly allocated block owning exactly
its own bytes, and the size-metadata `put` (`range_cache.rs:104`) builds a fresh
8-byte `Bytes`. No sub-slice of a large assembly buffer is ever stored.

## Implementation Steps

1. **`rust/object-cache/Cargo.toml`** — change the `bytes` dependency line to
   enable the `serde` feature: `bytes = { workspace = true, features = ["serde"] }`
   (keep alphabetical ordering; it stays in place).
2. **`rust/object-cache/src/foyer_backend.rs`**
   - Change the field type to `HybridCache<String, Bytes>`.
   - In `FoyerBackend::new`, no change is needed beyond type inference; confirm the
     `HybridCacheBuilder` chain still infers `Bytes` (it is inferred from the field
     type on assignment / return).
   - `get`: replace `Some(Bytes::from(entry.value().clone()))` with
     `Some(entry.value().clone())`.
   - `put`: replace `self.cache.insert(key, value.to_vec())` with
     `self.cache.insert(key, value)`.
3. **`rust/object-cache/Cargo.toml`** — add `tempfile = "3.14"` to
   `[dev-dependencies]` (matching the version already used in `analytics/Cargo.toml`
   and `analytics-web-srv/Cargo.toml`); it backs the round-trip test's temp disk-tier
   directory. Keep alphabetical ordering within the block.
4. Build with the foyer feature: `cargo build -p micromegas-object-cache --features foyer`
   and `cargo build -p micromegas-object-cache-srv`.
5. `cargo fmt`, then `cargo clippy -p micromegas-object-cache --features foyer -- -D warnings`.

## Files to Modify
- `rust/object-cache/Cargo.toml` — enable `bytes` `serde` feature; add
  `tempfile = "3.14"` to `[dev-dependencies]` for the round-trip test.
- `rust/object-cache/src/foyer_backend.rs` — store `Bytes`, drop both copies.
- `rust/object-cache/tests/` — new round-trip test (see Testing Strategy).

## Trade-offs

- **Enable `bytes/serde` vs. a thin newtype wrapper.** The issue suggests either
  confirming `Bytes` works or using a wrapper. Enabling the `bytes` serde feature is
  the smaller, well-supported path (no new type, no manual `Serialize`/`Deserialize`
  impl to maintain, disk format is a plain length-prefixed byte blob). A newtype
  would only be warranted if we needed a custom on-disk encoding, which we do not.
  Recommendation: enable the feature.
- **`Bytes` vs. `Arc<Vec<u8>>`.** `Arc<Vec<u8>>` would also make the RAM clone cheap,
  but `get` must return `Bytes`; producing a `Bytes` from `&Arc<Vec<u8>>` without a
  copy is awkward, whereas `Bytes` clones directly. `Bytes` is the natural fit and is
  already the trait's currency type.

## Coordination

Issue #1195 notes this touches the same `FoyerBackend` that #1203 reworks (where
Foyer becomes a pure `get`/`put` cache) and recommends doing it alongside #1203 to
avoid churn. This change is deliberately self-contained — it only swaps the stored
value type and removes copies, leaving the `get`/`put` shape intact — so it composes
cleanly with #1203 whether it lands before or as part of that work. No dependency on
#1203 is required to implement or verify it.

## Documentation
No user-facing documentation change. This is an internal performance change behind
the existing `RangeCacheBackend` interface. The `RangeCache` doc comment
(`range_cache.rs:32-42`) and the object-cache crate description need no update.

## Testing Strategy

There are currently no dedicated `FoyerBackend` tests (the `foyer` feature is
off by default and the disk tier needs a temp dir). Verification:

1. **Build + clippy** with `--features foyer` for both `micromegas-object-cache`
   and `micromegas-object-cache-srv` (the change must not regress the default,
   no-foyer build either — run `cargo build -p micromegas-object-cache` without the
   feature as well).
2. **Round-trip test** (add to `rust/object-cache/tests/`, gated on
   `#[cfg(feature = "foyer")]`): construct a `FoyerBackend` against a `tempfile`
   directory, `put` a multi-byte block under a key, `get` it back, and assert the
   returned `Bytes` equals the input. To actually exercise the disk serde/bincode
   path (the main new correctness risk), the read must miss the RAM tier: in foyer
   0.14.1 `insert` writes to memory and serializes to disk in the background, and a
   `get` right after `insert` is served from RAM without ever calling
   `storage.load`. So force a memory miss before reading by closing/reopening the
   cache so the read comes from disk. **Await the background flush before dropping
   the first cache**: `insert` returns immediately and enqueues the disk write to a
   background flusher, so if the cache is dropped before that flush completes the
   region scan on reopen finds nothing for the key and `get` misses (an
   intermittent failure). Call `HybridCache::close().await` on
   the first cache before dropping/reopening it. `FoyerBackend` currently exposes no
   such method, so the test must either add a way to await the flush on the wrapper
   or build the `HybridCache` directly. Disk recovery on reopen is on by default
   (`RecoverMode::Quiet`) and re-indexes prior entries, and files are opened without
   truncation, so the reopen path itself is sound once the flush has completed. Note
   that `ram_bytes` is not a byte budget
   here: `FoyerBackend::new` installs no weighter, and foyer's default per-entry
   weighter is `Arc::new(|_, _| 1)`, so `ram_bytes` acts as a max *entry count* — a
   single small `ram_bytes` will not evict one inserted block, and the read would
   still be served from RAM. Close/reopen is the reliable trigger (inserting more
   entries than the count-based capacity would also work). Only after a memory miss
   does the `bytes`/bincode deserialize half of the round-trip run. Use a small
   `ram_bytes`/`disk_bytes` so the test is cheap.
3. **Full CI**: `python3 build/rust_ci.py` from `rust/`.

## Open Questions
- None blocking. The `bytes` serde feature is confirmed available and Foyer's
  `StorageValue` bounds are satisfied; the change is mechanical.
