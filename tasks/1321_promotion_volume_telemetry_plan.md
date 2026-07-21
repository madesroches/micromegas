# Object-Cache disk→RAM Promotion Volume Telemetry Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1321

## Overview

Add two `{prefix}`-tagged metrics measuring disk→RAM **promotion volume** in the object cache:
`object_cache_promotion_count` (one per successful disk→RAM promotion) and
`object_cache_promotion_bytes` (the promoted block's length). Together with the existing
`object_cache_ram_tier_eviction_*` signals they form the RAM-tier **churn** picture: high promotion
+ high eviction at low residency age = the RAM tier is too small for the working set. Today only the
eviction half is visible. The #1318 two-step read already introduced the single site where a
validated block crosses disk→RAM — `promote_if_valid` — so this is a near-free two-line hook there.

## Current State

`#1318` (landed, `26b9daeaf`) reworked `FoyerBackend::get` into a two-step read: a plain RAM lookup,
then a direct `storage().load()` whose promotion into RAM is length-gated. The promotion is
funneled through one helper:

`rust/object-cache/src/foyer_backend.rs:362` — `promote_if_valid(cache, tags, key, value, age, expected_len) -> Option<Bytes>`:

- Called from both arms of `load_and_promote` (`:408`) — `Load::Entry` (`:417`) and `Load::Piece`
  (`:420`) — so it is the *single* disk→RAM crossing point for both.
- On a length mismatch (`value.bytes.len() != expected_len`) it emits
  `range_cache_promotion_len_mismatch` and returns `None` **without** promoting (`:370-376`).
- On a match, for block keys only it emits the tiered hit counter (`:377-380`):
  ```rust
  if is_block_key(key) {
      let t = tags.classify(key);
      imetric!("object_cache_disk_tier_hit", "count", t.prefix, 1_u64);
  }
  ```
  then optionally the disk read-age (`:381-393`), then inserts the fresh-normalized promotion record
  into the RAM tier via `cache.memory().insert_with_properties(...)` (`:395-399`) and returns the
  validated bytes. **This insert is the promotion.**

`is_block_key` (`:26`) matches `blk:`-prefixed keys. The `meta:{ns}:{key}` 8-byte size lookups
(`range_cache/mod.rs:201`) also flow through `get`/`load_and_promote` and *can* be promoted, but are
deliberately excluded from the tiered hit counters (doc comment `:21-28`) so the block-only miss-rate
derivation stays clean.

`EvictionTagTable` (`metric_tags.rs:157`) is already held by `FoyerBackend` (`:271`) and handed to
`promote_if_valid` as `tags`. `tags.classify(key)` returns `EvictionTags` whose `.prefix` field is a
precomputed `&'static PropertySet` for `{prefix}` — exactly the tag the two new metrics need, and
the one `disk_tier_hit` already uses.

**Prefix tag caveat (inherited, not fixed here):** for `blk:`-prefixed keys `classify` currently
always resolves to `"other"`, because the storage-prefixed `blk:...` key never matches a content
`prefix` label (`longest_prefix_match` sees the leading `blk:`). This is documented for the sibling
metrics at `mkdocs/docs/admin/object-cache.md:208` ("`{prefix}` currently always resolves to
`"other"` for these two … so only the aggregate is meaningful today"). The new promotion metrics
inherit the identical behavior: they are tagged by `{prefix}` for forward-consistency, so if the
`blk:` prefix classification is ever fixed they light up automatically, but today only the aggregate
is meaningful. This is intentional and matches `disk_tier_hit`.

Bytes-metric convention (`fetch.rs:402-407`): `imetric!("name", "bytes", tag, value as u64)`.

## Design

Extend the existing `is_block_key` branch in `promote_if_valid` — the same branch that already emits
`disk_tier_hit`, reusing the already-classified `t` — with the two promotion metrics:

```rust
if is_block_key(key) {
    let t = tags.classify(key);
    imetric!("object_cache_disk_tier_hit", "count", t.prefix, 1_u64);
    // Promotion volume (#1321). This length-validated insert below is the
    // single disk->RAM crossing, so promotion_count == disk_tier_hit by
    // construction; it exists as the named companion and denominator to
    // promotion_bytes (mean promoted block size = bytes / count) — the churn
    // signal weighed against object_cache_ram_tier_eviction_*.
    imetric!("object_cache_promotion_count", "count", t.prefix, 1_u64);
    imetric!(
        "object_cache_promotion_bytes",
        "bytes",
        t.prefix,
        value.bytes.len() as u64
    );
}
```

- **Placement.** Inside the existing `is_block_key` block, reusing `t` — no new `classify` call, no
  new field, no signature change. `value.bytes.len()` here equals `expected_len` (the length gate at
  `:370` already passed) and `value` is still owned at this point (the RAM insert at `:395` clones
  from it afterward), so the length read is valid and cheap.
- **Block-only gating (matches `disk_tier_hit`).** Gating on `is_block_key` keeps `meta:` 8-byte
  size-lookup promotions out of the volume signal. Including them would inflate the count by ~one
  per block and drag `promotion_bytes` toward 8 — noise for a "how much block data churns through
  RAM" metric. Keeping the promotion family block-scoped, exactly like the tiered hit counters it
  sits beside, is the consistent choice. See Trade-offs for why this (not eviction-parity) wins.
- **`promotion_count` ≡ `disk_tier_hit`.** Every successful block promotion is exactly one disk-tier
  hit, so the two counters are equal by construction. This is deliberate and documented inline:
  `promotion_count` is the named entry point to the promotion family and the natural denominator for
  `promotion_bytes`, whose per-promotion byte figure `disk_tier_hit` does not carry.
- **No length-mismatch counting.** A mismatch returns before the promotion insert (`:370-376`), so
  neither promotion metric fires for a poisoned-short entry — correct: no block actually crossed
  into RAM. `range_cache_promotion_len_mismatch` already covers that case.

No changes to `metric_tags.rs`, `EvictionTagTable`, function signatures, or wiring — the classifier
and the `tags` handle already reach the emission site.

## Implementation Steps

1. **`rust/object-cache/src/foyer_backend.rs`** — in `promote_if_valid`, extend the existing
   `if is_block_key(key) { … }` block (`:377-380`) with the `object_cache_promotion_count` and
   `object_cache_promotion_bytes` `imetric!` calls shown above, plus the explaining comment.
2. **`rust/object-cache/tests/foyer_backend_tests.rs`** — add a test asserting both metrics fire
   with a plausible value on a disk→RAM promotion (see Testing Strategy).
3. **`mkdocs/docs/admin/object-cache.md`** — document the two new metrics in the Monitoring table
   (see Documentation).
4. **`CHANGELOG.md`** — add an entry under the appropriate unreleased section.
5. Run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, and
   `cargo test -p micromegas-object-cache --features foyer`.

## Files to Modify

- `rust/object-cache/src/foyer_backend.rs` — two `imetric!` calls + comment in `promote_if_valid`.
- `rust/object-cache/tests/foyer_backend_tests.rs` — new promotion-volume test.
- `mkdocs/docs/admin/object-cache.md` — Monitoring-table rows for the two metrics.
- `CHANGELOG.md` — changelog entry.

## Trade-offs

- **Block-only gating vs. eviction-parity (ungated).** The issue frames promotion as the churn
  counterpart to `object_cache_ram_tier_eviction_*`, and the eviction listener is *not*
  `is_block_key`-gated (it fires for every non-prefetch key). A strict-parity reading would leave
  promotion ungated too. Rejected: (a) the volume question is about the *block* working set, and the
  metric family it visibly joins (`object_cache_{ram,disk}_tier_hit`) is uniformly block-scoped;
  (b) `meta:` promotions are 8-byte lookups that would inflate `promotion_count` by ~one per block
  and pull `promotion_bytes` toward 8, degrading the very volume signal the metric exists for; and
  (c) the eviction *age* signal is already dominated by blocks in practice (8-byte meta entries
  rarely drive capacity eviction), so block-gated promotion still lines up against the eviction
  signal operators actually read. Chosen: gate on `is_block_key`.
- **Emit `promotion_count` despite equalling `disk_tier_hit`.** It is redundant with the existing
  disk-hit counter by construction. Kept because the issue explicitly asks for a named promotion
  count, it is the semantic entry point to the promotion family (decoupled from the hit-rate
  family), and it is the clean denominator for `promotion_bytes` (mean promoted block size). The
  redundancy is called out inline so it reads as a deliberate choice, not an oversight.
- **Reuse the existing branch vs. a dedicated emission site.** Folding the two calls into the
  existing `is_block_key` block reuses the already-computed `t` and keeps a single classification per
  promotion (DRY), at the cost of `disk_tier_hit` and the promotion metrics sharing one `if`. Chosen
  — the alternative (a second `is_block_key`/`classify` pass) duplicates work for no benefit.

## Documentation

Add two rows to the Monitoring table in `mkdocs/docs/admin/object-cache.md` (near the
`object_cache_ram_tier_hit`/`disk_tier_hit` row at `:208` and the eviction rows), e.g.:

- `object_cache_promotion_count` (`+ prefix`) — one per successful disk→RAM block promotion (the
  length-validated `Load::Entry`/`Load::Piece` promote arms). Equal by construction to
  `object_cache_disk_tier_hit`; paired with `object_cache_promotion_bytes` as the disk→RAM churn
  volume, weighed against `object_cache_ram_tier_eviction_*`.
- `object_cache_promotion_bytes` (`+ prefix`) — bytes promoted disk→RAM (block length). With the
  count, gives mean promoted block size; the churn-volume half of the RAM-sizing signal.

Note the same `{prefix}`-resolves-to-`"other"`-for-`blk:` caveat already stated for the tiered hit
counters applies here (reference the existing sentence rather than restating it).

## Testing Strategy

Add `promotion_volume_metrics_fire_on_disk_read` to `foyer_backend_tests.rs`, modeled on
`disk_read_age_metric_fires_on_disk_read` (`:419-484`), which already sets up exactly this scenario
(put a block, force it to disk via RAM eviction, `close()` to flush, then `get` it back to trigger a
disk→RAM promotion) — with one required change: the promoted key must be `blk:`-prefixed (e.g.
`"blk:ns:blobs/key:0"`, matching the `blk:{ns}:{key}:{idx}` shape `range_cache/fetch.rs` builds),
since both new metrics are emitted inside `promote_if_valid`'s `is_block_key` branch and never fire
for a bare key like `"blobs/key"`. The warming/eviction-pressure puts (`"blobs/evict-1"`,
`"blobs/evict-2"`) don't need the `blk:` prefix — they only exist to push the measured key to disk:

- Reuse `tagged_integer_metric_values` (`:44`) for both metrics (both are integer `imetric`s —
  count and bytes).
- `get` must be called with the same `blk:`-prefixed key and its correct `expected_len` (the data
  length) to trigger the promotion.
- After the promoting `get`, `flush_metrics_buffer()`, then assert, for **`prefix="other"`** (not
  `"blobs"` — per the documented caveat, a `blk:`-prefixed key always classifies to `PREFIX_OTHER`
  since it never matches a content-label prefix):
  - `object_cache_promotion_count` fired exactly once (value `1`) for `prefix="other"`.
  - `object_cache_promotion_bytes` fired exactly once with value == the block length (4096 in that
    test's setup) for `prefix="other"`.
- Follow the existing test's `#[serial]` + `init_in_memory_tracing` guard usage (the guard is
  created *after* the warming puts and *before* the promoting `get`, so only the promotion is
  observed).

Optionally, extend `short_block_never_promoted` (`:493`) with a negative assertion that neither
promotion metric fired (a mismatch must not count as a promotion), reinforcing the length-gate
behavior. The short key used there must also be `blk:`-prefixed (e.g. `"blk:key"` in place of
`"key"`) — otherwise the assertion passes vacuously, since a non-`blk:` key never enters the
`is_block_key` branch regardless of whether the length gate fired, and the test would not actually
exercise the length-gate early-return. `short_block_never_promoted` currently has none of the
metrics-observation infra the positive test relies on, so extending it also requires adding: a
`#[serial]` attribute on the test fn, an `init_in_memory_tracing()` guard created after the durable
`put`/`close()` and before the mismatching `get("blk:key", full_len)` (mirroring
`disk_read_age_metric_fires_on_disk_read`'s guard placement), and a
`micromegas_tracing::dispatch::flush_metrics_buffer()` call right after that `get` and before
asserting on `guard.sink`. Without this infra the negative assertion would not actually observe
whether a metric fired. If that feels like too much surgery on an existing test, add a fresh
dedicated test instead, modeled on the same infra pattern, rather than extending
`short_block_never_promoted` in place.

Regression: existing `foyer_backend_tests` must pass unchanged — this adds emissions only, touching
no control flow. Run `cargo test -p micromegas-object-cache --features foyer` and
`cargo clippy --workspace -- -D warnings`.

## Open Questions

None.
