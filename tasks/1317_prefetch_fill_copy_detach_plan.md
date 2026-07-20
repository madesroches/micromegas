# object-cache Prefetch-Fill Copy Detach Plan

## Overview
`FoyerBackend::put`'s prefetch arm (`FillHint::Prefetch`) stores the incoming
`Bytes` slice verbatim, while the demand arm (`FillHint::Demand`) copies it
first. Both arms receive per-block slices carved out of a shared
coalesced-origin-GET buffer (up to `max_coalesced_get_bytes`, default 8 MiB),
so an unmodified prefetch slice keeps that whole parent allocation alive for
as long as the entry survives in foyer's disk-write pipeline (submission
channel, io buffer encode, `piece_refs` pending region reclaim) — which can be
minutes once the disk tier is under write pressure. None of this shows up in
`object_cache_ram_tier_usage_bytes` or any other exported gauge, since phantom
prefetch records have no RAM-tier residency. This is the same class of bug
fixed for the demand path in #1276, just missed on the prefetch arm. The fix
mirrors the demand arm exactly: copy the slice before handing it to foyer.

## Current State
`rust/object-cache/src/foyer_backend.rs:355-388` — `FoyerBackend::put`:
```rust
FillHint::Prefetch => {
    let entry = self
        .cache
        .storage_writer(key)
        .force()
        .insert(CachedBlock::new_prefetch(value));   // slice passed through, no copy
    ...
}
FillHint::Demand => {
    // Copy so the cached block does not retain its whole coalesced-GET
    // parent buffer; ...
    let owned = Bytes::copy_from_slice(&value);
    self.cache.insert(key, CachedBlock::new(owned));
}
```
The demand arm's comment already states the invariant the prefetch arm
violates. Both arms are fed the same kind of value: `fulfill_run_success`
(`rust/object-cache/src/range_cache/fetch.rs:403-412`) splits one coalesced
origin GET into per-block `Bytes::slice`s sharing the parent buffer, then
calls `self.backend.put(run.keys[i].clone(), chunk.clone(), hint)` for
whichever `hint` (`Demand` or `Prefetch`) the run was fetched under — the
split/slice logic itself is hint-agnostic, so the parent-retention exposure is
identical on both paths; only the backend's handling of the slice differs.

`rust/object-cache/src/bounded_memory_backend.rs:47-58` (the L1 backend) has
no demand/prefetch distinction and already copies unconditionally in `put` —
fixed alongside the original demand-path bug in #1276. This plan only touches
`FoyerBackend`.

## Design
Copy before insert in the prefetch arm, matching the demand arm's existing
pattern and comment style:

```rust
FillHint::Prefetch => {
    // Copy so the phantom prefetch record does not retain its whole
    // coalesced-GET parent buffer for the duration it lives in foyer's
    // write pipeline (submit queue, io buffer encode, pending piece_refs) --
    // see the demand arm's identical rationale below.
    let owned = Bytes::copy_from_slice(&value);
    let entry = self
        .cache
        .storage_writer(key)
        .force()
        .insert(CachedBlock::new_prefetch(owned));
    if entry.is_none() {
        imetric!(
            "range_cache_prefetch_admission_unexpected_none",
            "count",
            1_u64
        );
        warn!("prefetch storage_writer().force().insert() unexpectedly returned None");
    }
}
```

One memcpy per prefetched block, the same cost already accepted on the
demand path and negligible against the origin GET that produced the run.

No trait or call-site signature changes are needed — this is a self-contained
fix inside `FoyerBackend::put`.

## Implementation Steps
1. **Fix the leak** — `rust/object-cache/src/foyer_backend.rs`: copy the value
   in the `FillHint::Prefetch` branch of `put` before constructing
   `CachedBlock::new_prefetch`, with an explanatory comment mirroring the
   demand arm's.
2. **Test** — `rust/object-cache/tests/foyer_backend_tests.rs`: add the
   detachment regression test described below, and add
   `use std::sync::atomic::{AtomicBool, Ordering};` to the file's imports
   (it currently imports `std::sync::Arc` and `bytes::Bytes` but not the
   atomics the test uses).
3. Run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, and
   `cargo test -p micromegas-object-cache` (from `rust/`).

## Files to Modify
- `rust/object-cache/src/foyer_backend.rs` — copy on prefetch admission.
- `rust/object-cache/tests/foyer_backend_tests.rs` — detachment regression
  test for the prefetch path.

## Trade-offs
- **Copy per prefetch block vs. zero-copy slice.** Same trade-off already
  accepted for the demand path in #1276: one memcpy per admitted block is
  negligible next to the origin GET, and it makes the prefetch path's
  "does not retain RAM residency" invariant also true of the disk-write
  pipeline's transient retention, not just the RAM eviction structure.
- **No alternative considered.** The issue's own suggested fix (mirror the
  demand arm) is adopted verbatim — there is no simpler or cheaper way to
  detach the slice from its parent short of restructuring `fetch.rs` to
  originate owned buffers per block, which would spread the same copy cost
  across both backends and both hints instead of keeping it localized to the
  one backend that has a byte weigher relying on truthful retention (see
  #1276's "copy in backend vs. in `fetch.rs`" rationale, which applies
  identically here).

## Testing Strategy
The existing `prefetch_fill_lands_on_disk_not_ram` test
(`foyer_backend_tests.rs:122-153`) only asserts RAM-tier *accounted* usage
doesn't grow — it doesn't (and can't, via that metric) catch parent-buffer
retention, since phantom prefetch records never touch RAM-tier accounting
either way. A pointer-comparison test like the demand path's
`demand_fill_detaches_from_parent_buffer` also doesn't work here: prefetch
`get` always resolves through the disk tier's `Code::decode`, which allocates
a fresh buffer regardless of whether the fix is applied, so the assertion
would pass vacuously.

Instead, use a drop-tracking owner to directly observe whether the original
parent allocation is released once `put()` returns and the caller drops its
own reference — independent of when the (now-detached) copy's disk write
actually completes:

```rust
struct DropFlag {
    data: Vec<u8>,
    dropped: Arc<AtomicBool>,
}

impl AsRef<[u8]> for DropFlag {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

// A prefetch fill must be detached (copied) from its coalesced-GET parent
// buffer, or the async disk-write pipeline (submit queue, io buffer encode,
// pending piece_refs) keeps the whole parent allocation alive for as long as
// the entry is in flight -- see #1317.
#[tokio::test]
async fn prefetch_fill_detaches_from_parent_buffer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_str().expect("utf8 path");

    let backend = FoyerBackend::new_with_shards(
        dir_path,
        16 * 1024 * 1024,
        16 * 1024 * 1024,
        1,
        WriteTuning::default(),
        Arc::from(Vec::new()),
    )
    .await
    .expect("create backend");

    let dropped = Arc::new(AtomicBool::new(false));
    let parent = Bytes::from_owner(DropFlag {
        data: vec![7u8; 8192],
        dropped: dropped.clone(),
    });
    let block = parent.slice(0..4096);
    drop(parent);
    assert!(
        !dropped.load(Ordering::SeqCst),
        "sanity: the slice must still keep the owner alive"
    );

    backend
        .put("k".to_string(), block.clone(), FillHint::Prefetch)
        .await;
    drop(block);

    assert!(
        dropped.load(Ordering::SeqCst),
        "prefetch admission must copy, detaching the cached block from its \
         parent GET buffer instead of retaining a slice into it across the \
         async disk-write pipeline"
    );

    backend.close().await.expect("close backend");
}
```

This fails before the fix (the backend's internal `CachedBlock` still holds a
clone of `block`, so the shared owner isn't released when the test drops its
own references) and passes after (the backend holds only `owned`, an
independent allocation, so the original owner drops as soon as `block`'s last
external reference — the test's own — goes away).

`Bytes::from_owner` requires `bytes >= 1.9`; the workspace pins `bytes
1.11.1` (`rust/Cargo.toml:42`), so no dependency change is needed.

Also verify the existing `prefetch_fill_lands_on_disk_not_ram` and
`round_trip_with_custom_write_tuning` tests still pass unchanged (prefetch
round-trip behavior is unaffected; only the retained-allocation identity
changes).

Full gate: `cargo fmt`, `cargo clippy --workspace -- -D warnings`,
`cargo test -p micromegas-object-cache` (from `rust/`).

## Open Questions
None — the issue specifies the exact fix, and it is a narrow, mechanical
mirror of the already-shipped demand-path fix (#1276).
