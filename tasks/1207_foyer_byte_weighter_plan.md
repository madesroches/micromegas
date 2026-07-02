# FoyerBackend RAM Tier Byte Weighter Plan

## Overview
`FoyerBackend::new` builds a foyer `HybridCache` with `.memory(ram_bytes)` but
installs no weighter. foyer's default per-entry weight is `1`, so `.memory(cap)`
is a **max entry count**, not a byte budget. The production caller
(`object-cache-srv`) passes a byte value (`ram_mb * 1024 * 1024`) into that
count, so the intended 512 MB RAM bound is actually interpreted as ~537M
*entries* — the RAM tier is effectively unbounded in bytes before it evicts,
which is an OOM risk under sustained load. This plan installs a byte weighter so
`.memory()` becomes a true byte budget, matching the caller's intent.

## Current State
- `rust/object-cache/src/foyer_backend.rs:14-22` — `FoyerBackend::new(dir, ram_bytes, disk_bytes)`
  builds the hybrid cache with no `.with_weighter(...)` call:
  ```rust
  let cache = HybridCacheBuilder::new()
      .memory(ram_bytes)
      .storage(Engine::Large)
      .with_device_options(DirectFsDeviceOptions::new(dir).with_capacity(disk_bytes))
      .build()
      .await?;
  ```
- `rust/object-cache-srv/src/object_cache_srv.rs:76-82` — the production caller
  passes a byte budget into the entry-count parameter:
  ```rust
  let foyer = FoyerBackend::new(
      &args.disk_path,
      args.ram_mb * 1024 * 1024,   // bytes, but treated as an entry count
      args.disk_gb * 1024 * 1024 * 1024,
  ).await ...
  ```
  `ram_mb` defaults to `512` (`cli.rs:18-19`), so the RAM capacity is
  `536_870_912` — interpreted as entries, not bytes.
- `rust/object-cache/tests/foyer_backend_tests.rs:20-47` — `round_trip_through_disk_tier`
  relies on the current "capacity = entry count" semantics: it passes `ram_bytes = 1`
  and a comment (lines 26-30) explains that capacity `1` means one entry, so each
  subsequent `put` evicts the previous entry to disk. This test and comment must
  change when the weighter is installed.

### foyer API (0.14.1, confirmed)
- `HybridCacheBuilder::new().memory(capacity)` → `HybridCacheBuilderPhaseMemory<K, V, S>`,
  which exposes `.with_weighter(weighter: impl Weighter<K, V>)`
  (`foyer-0.14.1/src/hybrid/builder.rs:177`).
- `Weighter<K, V>: Fn(&K, &V) -> usize` (`foyer-memory-0.14.1/src/raw.rs:58`).
- For `HybridCache<String, Bytes>`, a `|_key, value: &Bytes| value.len()` closure
  satisfies the trait directly (`Bytes::len()` returns `usize`).

## Design
Install a byte weighter that returns the payload size, so the memory tier's
capacity is measured in bytes and the existing `ram_mb * 1024 * 1024` caller
becomes correct with no caller change.

In `FoyerBackend::new`:
```rust
let cache = HybridCacheBuilder::new()
    .memory(ram_bytes)
    .with_weighter(|_key: &String, value: &Bytes| value.len())
    .storage(Engine::Large)
    .with_device_options(DirectFsDeviceOptions::new(dir).with_capacity(disk_bytes))
    .build()
    .await?;
```

`ram_bytes` now genuinely bounds resident payload bytes in the RAM tier. The
parameter name `ram_bytes` already reflects this intended meaning, so no rename
is needed. (The weight counts only the `Bytes` payload, not the `String` key or
foyer's per-entry overhead, so real RSS will run modestly above `ram_bytes`;
this matches how foyer byte budgets are normally used and is a large improvement
over the effectively-unbounded status quo.)

Note on sharding: foyer's memory tier defaults to 8 shards and divides
`capacity` per shard (`shard_capacity = capacity / shards`) with eviction
running per shard, so `ram_bytes` is a **total** byte budget spread across shards
rather than a single flat bucket. This is fine for the 512 MB production bound.
It does, however, break a small-capacity test: a byte payload is only evicted
when a later key happens to hash into the same shard, and shard selection uses a
per-process randomly-seeded `ahash::RandomState`, so eviction of any particular
key is non-deterministic run-to-run. To make the test deterministic we expose a
constructor that builds the memory tier with a single shard, so the byte budget
is one bucket and the first over-budget put evicts `"key"`. Production keeps
foyer's default sharding.

Extract the builder into `new_with_shards(dir, ram_bytes, disk_bytes, shards)`
which adds `.with_shards(shards)` after `.with_weighter(...)`; `new` delegates
with foyer's default of `8` shards (equivalent to the current unset behavior),
and the test uses `1`.

### Test update
`round_trip_through_disk_tier` currently depends on `ram_bytes = 1` meaning "1
entry". Under the byte weighter, `1` byte is smaller than any real payload, which
would make every insert exceed capacity. Because foyer's memory tier defaults to
8 shards and evicts per shard (with a per-process random shard seed), a small
flat capacity would not deterministically evict `"key"` — the entry only leaves
the RAM tier when a later key happens to hash into its shard. Build the backend
with a single-shard memory tier (via `new_with_shards(..., 1)`) so the byte
budget is one bucket, then set the RAM capacity to exactly hold one 4096-byte
payload: the first `put("key", 4096 bytes)` fits and the following puts push the
tier over the byte budget, deterministically evicting `"key"` to disk:

```rust
// ram_bytes is a byte budget (a value.len() weighter is installed) and the
// memory tier uses a single shard, so the budget is one bucket: capacity 4096
// exactly holds the first 4096-byte payload, so the subsequent puts push the
// RAM tier over budget and evict "key", which enqueues it for the disk tier
// (the disk write is triggered by memory eviction, not by insert itself).
let backend = FoyerBackend::new_with_shards(dir_path, 4096, 16 * 1024 * 1024, 1)
    .await
    .expect("create backend");
```

The rest of the test (put/close/get, `close()` awaiting the flusher) is unchanged.

## Implementation Steps
1. `rust/object-cache/src/foyer_backend.rs`: add `.with_weighter(|_key: &String, value: &Bytes| value.len())`
   immediately after `.memory(ram_bytes)`. `Bytes` is already imported (line 3).
   Extract the builder into `new_with_shards(dir, ram_bytes, disk_bytes, shards)`
   that adds `.with_shards(shards)` after the weighter, and make `new` delegate to
   it with the foyer default of `8` shards.
2. `rust/object-cache/tests/foyer_backend_tests.rs`: switch the constructor call to
   `FoyerBackend::new_with_shards(dir_path, 4096, 16 * 1024 * 1024, 1)` and rewrite
   the comment (lines 26-30) to describe single-shard byte-budget eviction as above.
3. From `rust/`: run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, and
   `cargo test -p micromegas-object-cache --features foyer` (the test module is
   gated on `#![cfg(feature = "foyer")]`).

## Files to Modify
- `rust/object-cache/src/foyer_backend.rs` — install byte weighter; add
  `new_with_shards` helper and delegate `new` to it (default 8 shards).
- `rust/object-cache/tests/foyer_backend_tests.rs` — use `new_with_shards(..., 1)`
  + update comment.

## Trade-offs
- **Chosen: install a byte weighter.** Makes the existing `ram_mb * 1024 * 1024`
  caller correct with a one-line change and no caller/env-var churn. The parameter
  is already named `ram_bytes`, so semantics and name align.
- **Alternative: rename to an explicit entry count** and have the caller pass a
  count instead of a byte value. Rejected: the caller and operators reason about
  RAM in megabytes (`MICROMEGAS_OBJECT_CACHE_RAM_MB`), so a byte budget is the
  natural unit; an entry count would force the caller to guess an average block
  size to bound memory, which is exactly the footgun this issue is about.
- **Weight excludes key + foyer overhead.** Accepted: block payloads dominate
  (default `block_size` = 1 MiB) and the small constant per-entry overhead is
  negligible relative to the byte budget; the previous behavior was unbounded, so
  any true byte bound is strictly better.

## Documentation
No dedicated docs page covers `FoyerBackend` internals. The `ram_mb` semantics
are described only in code comments and the CLI help; the CLI help text
(`cli.rs:18`, `MICROMEGAS_OBJECT_CACHE_RAM_MB`) already implies a memory-size
budget, which becomes accurate after this change. No doc updates required.

## Testing Strategy
- Update and run `round_trip_through_disk_tier` (feature `foyer`) to confirm the
  disk round-trip still works with a byte-budget RAM tier.
- `cargo clippy --workspace -- -D warnings` to confirm the closure type-checks
  against the `Weighter` bound.
- Optional manual sanity check: run `object-cache-srv` with a small
  `MICROMEGAS_OBJECT_CACHE_RAM_MB` and drive enough distinct range reads to
  exceed it, confirming RSS plateaus near the configured budget rather than
  growing unbounded.

## Open Questions
None. The issue names the byte-weighter fix as the option that makes the current
caller correct; this plan implements it.
