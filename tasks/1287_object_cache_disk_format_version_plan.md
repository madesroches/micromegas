# Object-Cache Disk Store Format Version Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1287

## Overview

The object-cache foyer disk tier has no on-disk **format version**. When a build whose serialized
value layout differs from the one that wrote a persisted disk store starts against that store, foyer
recovers the old entries and hands them to the new `CachedBlock::decode`, which misframes them. Most
misdecodes surface as a foyer "coding error" → treated as a miss → safe origin fallback (but the disk
tier is dead and extremely noisy until fully overwritten). A narrow subset misdecodes into a
*plausible-but-wrong* 8-byte value that slips past the `data.len() == 8` guard in
`RangeCache::size()`; `decode_size` then accepts a garbage `file_size` (observed:
`5770522986613457232`, i.e. the bytes `PAR1…`, a Parquet header read as a `u64`), which drives a
multi-exabyte buffer allocation and **panics the process** — a crash loop on every restart.

This plan gives the disk store a **format-version guard** that reuses a single fixed directory and
wipes it in place only when the format actually changes, plus a **defensive size sanity-check** so a
corrupt cache entry can degrade to a miss but never panic the server.

## Current State

### Disk store construction (foyer 0.22.3)

`rust/object-cache/src/foyer_backend.rs` builds the disk tier with `FsDeviceBuilder::new(dir)` pointed
directly at the operator-supplied `--disk-path` (`object-cache-srv/src/cli.rs:27`,
`MICROMEGAS_OBJECT_CACHE_DISK_PATH`, required, persistent). `new_with_shards` (`foyer_backend.rs:192`)
builds:

```rust
let device = FsDeviceBuilder::new(dir).with_capacity(disk_bytes).with_direct(true).build()?;
let cache = HybridCacheBuilder::new()
    .with_event_listener(listener)
    .memory(ram_bytes)
    .with_weighter(...)
    .with_shards(shards)
    .with_eviction_config(LruConfig::default())
    .storage()
    .with_engine_config(BlockEngineConfig::new(device)...)
    .build().await?;
```

foyer recovers the store on startup (`RecoverMode::Quiet` default — recovers what it can, logging
quietly), so pre-existing entries are read back and decoded. `FsDevice` writes only files prefixed
`foyer-storage-direct-fs-` (`foyer-storage-0.22.3/src/io/device/fs.rs`), and `FsDeviceBuilder::build`
already `create_dir_all`s the directory.

### The value layout with no version (`CachedBlock`)

`#1283` changed the value type from `Bytes` to `CachedBlock` and gave it a custom `Code` impl
(`foyer_backend.rs:66-90`) that prepends an 8-byte `i64` LE `disk_write_ms` timestamp:

```rust
fn encode(&self, writer) { chrono::Utc::now().timestamp_millis().encode(writer)?; self.bytes.encode(writer) }
fn decode(reader) { let disk_write_ms = i64::decode(reader)?; let bytes = Bytes::decode(reader)?; ... }
```

A store written by the pre-`#1283` `Bytes`-only layout (which had no timestamp prefix) misframes under
this `decode`: the first 8 payload bytes are consumed as `disk_write_ms`, then `Bytes::decode` reads a
bogus length prefix off real payload bytes. Nothing prevents a new binary from decoding an
incompatible old store; there is **no version marker, magic, or wipe**.

### The size fast-path — where a misdecode becomes a panic

`RangeCache::size()` (`rust/object-cache/src/range_cache/mod.rs:191-259`) first probes the backend for
a cached size:

```rust
if let Some(data) = self.backend.get(&meta_key).await
    && data.len() == 8
{
    return decode_size(&data);   // trusts any 8-byte value as a u64 size
}
```

`decode_size` (`rust/object-cache/src/range_cache/scheduler.rs:432`) is:

```rust
pub(super) fn decode_size(data: &Bytes) -> Result<u64> {
    Ok(u64::from_le_bytes(data[..8].try_into().expect("8-byte size slice")))
}
```

The `data.len() == 8` guard rejects a full block, but a misdecode that yields exactly 8 bytes passes.
The resulting garbage `file_size` flows into buffer allocations (e.g.
`collect_ranges_from_stream`'s `BytesMut::with_capacity(need)` at `mod.rs:559`, `assemble_range`'s
`Vec::with_capacity` at `blocks.rs:52`) that abort the process. The block-fetch path already heals a
length-mismatched cached block (`fetch.rs:176-187`) and validates origin-run lengths
(`fetch.rs:370-385`); the **size** path is the unguarded gap.

### The foyer hash builder is already cross-run stable (correcting a prior assumption)

An earlier finding (from foyer **0.14**) held that foyer's default hash builder used a per-instance
random ahash seed, so disk-tier key lookups missed 100% after a restart. **This no longer holds on
0.22.3**: `HybridCacheBuilder::memory()` returns a phase parameterized by
`DefaultHasher = BuildHasherDefault<XxHash64>` (`foyer-0.22.3/src/hybrid/builder.rs:115`,
`foyer-common-0.22.3/src/code.rs:35`), which foyer documents as *"guaranteed that the hash results of
the same key are the same across different runs."* So after `#1228` the disk tier's key lookups
already survive restarts — and that stability is precisely what makes the misdecode reachable
(old-format entries are *found* by key and fed to the new decoder, rather than harmlessly missing).
A `with_hash_builder(...)` override is therefore **not** part of this fix; see Non-Goals. The stale
comment in `object-cache/tests/foyer_backend_tests.rs:59-70` (still describing the 0.14 random-seed
behavior) should be corrected as part of this work.

## Design

### Part 1 — Format-version guard, in place (primary fix)

Persist a version marker next to the store and reuse a **single fixed directory**, wiping it in place
only on a version mismatch. This reuses the same on-disk capacity across every restart (no `vN`
subdirectory accumulation and no transient 2× headroom), and takes the one-time cold start only when
the format actually changes.

**Version constant** — in `foyer_backend.rs`, next to `CachedBlock`'s `Code` impl:

```rust
/// On-disk format version for the foyer disk tier. The serialized value layout
/// (`CachedBlock`'s `Code` impl) carries no self-describing version, so a layout
/// change would otherwise misdecode entries recovered from a persisted store on
/// restart (see #1287, #1283). On startup the store directory is wiped iff the
/// persisted marker does not match this constant.
///
/// BUMP THIS whenever `CachedBlock`'s `Code` encode/decode (or any on-disk
/// layout foyer persists for us) changes.
///
/// History:
/// - v1: `CachedBlock` = `[i64 LE disk_write_ms][length-prefixed Bytes]` (#1283).
///   (The pre-#1283 `Bytes`-only layout was unversioned; upgrading onto a store
///   it wrote is the crash this guard prevents.)
const DISK_FORMAT_VERSION: u32 = 1;

/// Marker filename holding the decimal `DISK_FORMAT_VERSION`, stored alongside
/// foyer's own `foyer-storage-direct-fs-*` region files inside `--disk-path`.
/// The name does not collide with foyer's prefix, so foyer's recovery ignores it.
const DISK_FORMAT_MARKER: &str = "micromegas-object-cache-format-version";
```

**Startup routine** — a helper run at the top of `new_with_shards`, before building the device:

```rust
fn prepare_disk_dir(dir: &str, version: u32) -> Result<()> {
    let dir_path = std::path::Path::new(dir);
    let marker = dir_path.join(DISK_FORMAT_MARKER);
    let current = std::fs::read_to_string(&marker).ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    if current == Some(version) {
        return Ok(()); // match: let foyer recover the store untouched (warm reuse)
    }
    // Missing marker (first boot, or a pre-versioning old-format store) or a
    // mismatch: reclaim the space and start clean on the SAME directory.
    if dir_path.exists() {
        warn!(
            "object-cache disk format {current:?} != {version}; wiping {dir} to avoid \
             misdecoding old-format entries (#1287)"
        );
        // Remove directory CONTENTS, not the directory itself, so a mounted
        // volume root is preserved.
        for entry in std::fs::read_dir(dir_path)
            .with_context(|| format!("reading disk dir {dir}"))?
        {
            let path = entry?.path();
            if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) }
                .with_context(|| format!("removing {}", path.display()))?;
        }
    } else {
        std::fs::create_dir_all(dir_path).with_context(|| format!("creating disk dir {dir}"))?;
    }
    std::fs::write(&marker, version.to_string())
        .with_context(|| format!("writing disk format marker {}", marker.display()))?;
    Ok(())
}
```

Then `new_with_shards` calls `prepare_disk_dir(dir, DISK_FORMAT_VERSION)?;` before
`FsDeviceBuilder::new(dir)`.

**Ordering / crash-safety.** Read marker → (match → return, no wipe) / (mismatch → wipe contents +
write marker) → build device (foyer `create_dir_all`s and recovers; on a wiped dir there is nothing to
recover → clean cold start). The wipe frees space *before* foyer allocates, so there is never a 2×
spike. A crash between wipe and marker-write is self-healing: the next boot sees a missing/old marker,
re-wipes an already-empty dir, and rewrites the marker (idempotent). On the first deploy of this
change the existing top-level old-format `foyer-storage-direct-fs-*` files have no marker → treated as
mismatch → wiped and reclaimed.

`imetric!("object_cache_disk_format_wiped", "count", 1)` on the wipe branch makes a format-changing
deploy observable.

### Part 2 — Defensive size sanity-check (defense-in-depth)

Part 1 removes *this* trigger, but `size()`/`decode_size` still trust any 8-byte backend value as a
size, so any future corruption (bit rot, a foyer bug, a hash collision) that yields a bogus 8-byte
value re-arms the panic. Add a plausibility ceiling and degrade to a miss instead of crashing.

In `range_cache/mod.rs` (near `size()`, its sole consumer), define a generous ceiling:

```rust
/// Upper bound on a plausible cached object size. No micromegas lake object
/// (parquet partition or blob) approaches this; a decoded size above it means a
/// corrupt/misdecoded cache entry, which is treated as a miss and re-resolved
/// from origin rather than driving a catastrophic allocation (#1287).
pub const MAX_PLAUSIBLE_OBJECT_SIZE: u64 = 1 << 48; // 256 TiB
```

Restructure the `size()` fast path (`mod.rs:198-204`) so an implausible decode falls through to the
origin HEAD (which re-HEADs and overwrites the poisoned meta entry) instead of returning it:

```rust
if let Some(data) = self.backend.get(&meta_key).await
    && data.len() == 8
{
    let size = decode_size(&data)?;
    if size <= MAX_PLAUSIBLE_OBJECT_SIZE {
        imetric!("range_cache_size_backend_hit", "count", prefix_tag, 1_u64);
        return Ok(size);
    }
    imetric!("range_cache_size_implausible", "count", prefix_tag, 1_u64);
    warn!("range_cache implausible cached size {size} for key={key}; treating as miss");
    // fall through to origin HEAD, which repopulates meta:{key}
}
```

`decode_size` itself stays as-is (the ceiling lives at the call site so the fast path can choose to
miss rather than error). No other allocation site needs changing: the origin HEAD returns the true
`object_meta.size` and repopulates the entry, so the tier self-heals.

### Non-Goals

- **No `with_hash_builder(...)` override.** foyer 0.22.3's default `DefaultHasher` is already
  cross-run stable (see Current State), so warm restarts already work; adding a fixed hash builder
  would be a no-op. Explicitly called out because an earlier draft of this fix proposed it based on
  the now-obsolete 0.14 behavior.
- **No per-version `vN` subdirectory.** Rejected in favor of in-place reuse (see Trade-offs) so
  usable capacity does not erode with upgrades.
- **No content-integrity checking of block bytes.** foyer's own per-entry checksums cover on-disk
  corruption; the format-misdecode class is fixed by the version guard. Wrong-but-correct-length
  block *content* is out of scope.

## Implementation Steps

1. **`rust/object-cache/src/foyer_backend.rs`**
   - Add `DISK_FORMAT_VERSION` and `DISK_FORMAT_MARKER` here (`MAX_PLAUSIBLE_OBJECT_SIZE` goes in
     `range_cache/mod.rs` — see Step 2).
   - Add `prepare_disk_dir(dir, version) -> Result<()>` and call it at the top of `new_with_shards`
     before `FsDeviceBuilder::new`.
   - Add `use anyhow::Context;` (already imports `Result, ensure`) and `use std::path::Path;`.
   - Emit `object_cache_disk_format_wiped` on the wipe branch.
2. **`rust/object-cache/src/range_cache/mod.rs`** — apply the size-fast-path plausibility check;
   import `MAX_PLAUSIBLE_OBJECT_SIZE`; add the `range_cache_size_implausible` metric.
3. **`rust/object-cache/tests/foyer_backend_tests.rs`** — correct the stale hash-builder comment
   (`:59-70`) to describe foyer 0.22's stable `XxHash64` default; add the format-guard tests below
   (first-boot marker write, same-version reuse, mismatch wipes, missing marker wipes, directory
   preserved).
4. **`rust/object-cache/tests/range_cache_tests.rs`** — add the implausible-size test below, alongside
   the existing `size()` tests (e.g. `size_returns_file_size`). No `foyer` feature needed; uses
   `MemoryBackend`.
5. **Docs** — `mkdocs/docs/admin/object-cache.md`: note the format-version wipe-on-mismatch behavior
   under the disk-path/`MICROMEGAS_OBJECT_CACHE_DISK_PATH` description, and list the two new metrics.
6. **Full gate** — `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test` from `rust/`
   (with the `foyer` feature for the srv + foyer tests), then `python3 build/rust_ci.py`.

## Files to Modify

- `rust/object-cache/src/foyer_backend.rs`
- `rust/object-cache/src/range_cache/mod.rs`
- `rust/object-cache/tests/foyer_backend_tests.rs`
- `rust/object-cache/tests/range_cache_tests.rs`
- `mkdocs/docs/admin/object-cache.md`

(No CLI/env changes: `--disk-path` semantics are unchanged; the version is a compile-time constant.)

## Trade-offs

- **In-place wipe on a single fixed dir vs. per-version `vN` subdirectory.** A `vN` subdir either
  accumulates dead generations (leaking disk) or needs transient 2× headroom, eroding effective
  capacity with each upgrade. In-place reuse keeps one directory forever and takes the destructive
  wipe exactly once per format change. Chosen per the requirement that disk space be reused rather
  than shrink across updates.
- **Wipe on mismatch vs. graceful per-entry version negotiation.** foyer offers no per-entry version
  hook, and the disk tier is a pure cache (no data loss, no correctness impact from a cold start), so
  a whole-store wipe on a format change is simpler and fully robust. The cost is a one-time rewarm
  from origin after a format-changing deploy.
- **Removing dir *contents* vs. `remove_dir_all(dir)` + recreate.** Removing contents preserves the
  directory (and any mount point rooted there); `remove_dir_all` on a mount root would fail. Slightly
  more code, but safe regardless of how `--disk-path` is provisioned.
- **Version marker as a plain file inside `--disk-path` vs. a nested `store/` subdir.** foyer only
  touches `foyer-storage-direct-fs-*` files, so a differently-named marker is ignored by recovery and
  needs no layout change (avoids orphaning the existing top-level region files on first deploy).
- **Plausibility ceiling vs. guarding every allocation site.** A single ceiling at the `size()`
  chokepoint (DRY) degrades a corrupt entry to a miss; peppering `with_capacity` sites would be
  invasive and easy to miss one. The constant is deliberately far above any real object so it never
  rejects a legitimate size.
- **Defensive check kept in this plan vs. split out.** It is logically independent of the version
  guard and could ship separately; it is included here because the same issue exposed it and it is the
  guard against the panic recurring for any *other* corruption source. Easy to drop to a follow-up if
  preferred (see Open Questions).

## Documentation

`mkdocs/docs/admin/object-cache.md`:
- Under `MICROMEGAS_OBJECT_CACHE_DISK_PATH` / `--disk-path`, add a short note: the disk store carries
  an internal format version; a build whose format differs from the persisted store wipes the store
  directory once on startup and rewarms from origin (no data loss). Same-format restarts reuse the
  store warm.
- Add `object_cache_disk_format_wiped` (count; fires once per format-changing startup) and
  `range_cache_size_implausible` (count; a corrupt cached size rejected and re-resolved from origin)
  to the metrics reference.

## Testing Strategy

The format-guard tests below live in `object-cache/tests/foyer_backend_tests.rs` (gated on the
`foyer` feature), following the existing eviction-based disk round-trip pattern and its
no-fixed-`sleep` guidance. The size-plausibility test needs no `foyer` feature and goes in
`object-cache/tests/range_cache_tests.rs` instead, alongside the existing `size()` tests:

- **First-boot marker write.** `new_with_shards` on a fresh tempdir → assert the marker file exists
  and contains `DISK_FORMAT_VERSION`.
- **Same-version reuse (no wipe).** Build a backend on a tempdir, force a RAM→disk eviction, `close()`;
  build a second backend on the *same* dir → assert the pre-existing
  `foyer-storage-direct-fs-*` files were **not** removed (the marker matched) and the marker is
  unchanged. (Whether the key is re-hit is foyer's concern; this asserts the wipe did not run.)
- **Mismatch wipes and reclaims.** Write a stale marker (`0`) plus a dummy
  `foyer-storage-direct-fs-00000000` file into a tempdir, then `new_with_shards` → assert the dummy
  file is gone, the marker now reads `DISK_FORMAT_VERSION`, and the backend builds and serves a fresh
  put/get. Simulates the crashing upgrade.
- **Missing marker (pre-versioning store) wipes.** Same as above but with no marker file present.
- **Directory (mount) preserved.** After a wipe, assert the disk dir itself still exists (contents
  removed, not the directory).
- **Implausible size degrades to a miss (`range_cache/mod.rs`).** In
  `object-cache/tests/range_cache_tests.rs` (no `foyer` feature needed — same module as
  `size_returns_file_size`), unit-drive `RangeCache::size()` built via `make_cache` over an `InMemory`
  backend and a `CountingStore` origin, with `meta:{ns}:{key}` pre-seeded so it holds 8 bytes decoding
  above `MAX_PLAUSIBLE_OBJECT_SIZE` → assert `size()` returns the origin's real size (not the poisoned
  value) and does not panic.
- **Full gate** as in Implementation Step 5.

## Open Questions

_All resolved._

1. **Constant placement — resolved.** `MAX_PLAUSIBLE_OBJECT_SIZE` lives in `range_cache/mod.rs` near
   `size()`, its sole consumer. Not promoted to a shared consts module (no second caller; YAGNI).
2. **Ceiling value — resolved.** `1 << 48` (256 TiB). The whole disk tier defaults to
   `MICROMEGAS_OBJECT_CACHE_DISK_GB=50` (50 GiB), and individual lake objects (parquet partitions,
   blobs) are MB–low GB, so 256 TiB is orders of magnitude above any real object and cannot reject a
   legitimate size while still catching the observed ~5 EiB garbage.
3. **Bundle vs. split — resolved.** Ship both parts in one change, as the issue lists both and they
   share the same test file and gate. If review prefers single-purpose PRs, Part 2 (size check) is
   cleanly separable, but the default is to bundle.
