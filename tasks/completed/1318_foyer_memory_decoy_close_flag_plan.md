# foyer-memory Decoy Close-Flag — Fix Design (validated-promotion two-step read)

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1318

> **Status (2026-07-20): implemented (checklist items 1-8). Item 9 (update issue, file upstream bug) is a follow-up outside this PR.**
> The fix is entirely in object-cache (no vendoring, no fork, no `[patch]`): replace
> `HybridCache::get` with a two-step read (RAM lookup, then direct disk load) whose
> disk→RAM **promotion is gated on a caller-supplied expected length**, plus our own
> per-key single-flight around the disk load to keep read-coalescing parity. This makes
> foyer's buggy inflight path **unreachable** and makes every remaining write race
> **provably byte-identical**, at no performance cost.

## Framing: a read-path improvement, not a workaround

The test that justifies this change: **it stays even after upstream fixes the bug**
(checklist 9). A workaround you'd delete the moment a fixed foyer ships would be pure
overhead against a benign, self-healing race — that cost/benefit was the case for the
earlier accept-as-benign resolution. This design passes a stronger test: judged on its
own merits — validation-based safety instead of cancellation timing, strictly less
machinery than foyer's inflight route, and the tiered-hit telemetry — it is a net
improvement to the read path whose *urgency* happens to come from the bug. The one
ongoing cost either way is the pinned-composition coupling: on each foyer upgrade,
check whether hybrid promotion semantics changed (the `Age` handling in the verified
facts below being the known example).

## The bug (root cause)

`foyer-memory` 0.22.3 has a bug in `InflightManager::enqueue`'s `Entry::Vacant` arm
(`src/inflight.rs`, ~L206-228): the fetch leader is handed a *different*
`Arc<AtomicBool>` "close" flag from the one stored in the inflight table.

```rust
Entry::Vacant(v) => {
    ...
    let entry = InflightEntry {
        ...
        inflight: Inflight {
            close: Arc::new(AtomicBool::new(false)),   // Arc A -- stored in the table
            ...
        },
    };
    v.insert(entry);
    let close = Arc::new(AtomicBool::new(false));       // Arc B -- a DIFFERENT Arc
    Enqueue::Lead { id, close, .. }                     // leader gets Arc B
}
```

Callers that cancel an inflight fetch (`take`, called by every `insert` via
`raw.rs::emplace`) set the **table's** copy (Arc A); the leader's `RawFetch::poll`
checks its **own** copy (Arc B, raw.rs ~L1299/L1316), which nothing ever writes.
Cancellation is inert. Consequence in object-cache: a demand `insert()` (fresh bytes)
racing an in-flight disk read of the same key cannot cancel it; the leader finishes and
**re-inserts the disk value over the fresh insert**. The leader is a *detached spawned
task* (`spawner.spawn(fetch)`, raw.rs:1034), so it outlives any caller scope.

**Upstream status (verified 2026-07-20):** 0.22.3 (2026-01-23) is the newest release on
crates.io and `foyer-rs/foyer` `main` still has the identical two-Arc bug. No upstream
fix exists. The one-line fix (share a single Arc) should still be reported upstream.

## Why the clobber can hurt at all

All object-cache keys are write-once/content-addressed, so cached bytes are immutable
per key — a clobber is normally byte-identical (wasted work, not corruption). The single
differing-bytes case is the **poisoned short prefetch**: a prefetch stored a block under
an undersized caller-supplied `size`, leaving short bytes on disk. `probe_blocks`
(`range_cache/fetch.rs:176`) detects the length mismatch, refetches from origin, and
heals via `put(Demand)` — and a concurrent zombie disk read of the short block can
re-insert it over the healed bytes. (Self-healing: the next probe re-heals. Also
verified: `size()` (`range_cache/mod.rs:204`) validates 8-byte length + plausibility, so
no consumer ever *serves* clobbered bytes — today's impact is a rare extra origin
refetch, not corruption.)

**Key insight the fix is built on:** the only bytes that must never be (re-)written into
the cache are *short* bytes — and the callers always know the exact expected length. If
every disk→RAM promotion is validated against that expected length before writing, then
every RAM write in the system carries the key's one canonical full-length byte string,
so **any** write-write race is byte-identical and therefore harmless — no cancellation
needed at all.

## The fix

### 1. `RangeCacheBackend::get` takes the expected length

```rust
async fn get(&self, key: &str, expected_len: u64) -> Option<Bytes>;
```

Contract: `expected_len` is the exact length the caller will accept. A backend MUST NOT
copy a value whose length differs into any faster tier (promotion gate), and treats such
a value as a miss. Both existing callers already know it:

- `probe_blocks` (`range_cache/fetch.rs`) computes `expected_len` from
  `block_byte_range` today, *after* awaiting the get (lines 176-178). To pass it into
  `get`, that `block_byte_range`/`expected_len` computation moves earlier, into the
  eagerly-collected probe closure ahead of the `backend.get` call (`idx`, `file_size`,
  and the `Copy` `block_size` are all capturable there); the post-get length check is
  retained as defense in depth.
- `size()` (`range_cache/mod.rs:204`) passes `8`.

`MemoryBackend` / `BoundedMemoryBackend` (single-tier, no promotion) accept and ignore
the parameter.

### 2. `FoyerBackend::get` becomes a two-step read — no foyer inflight path

Replace `self.cache.get(key).await` (which routes through `memory.get_or_fetch_inner` →
`InflightManager::enqueue`, creating the buggy leader) with the public non-inflight
APIs. Verified against foyer 0.22.3 sources — `HybridCache::get` is *exactly* this
composition internally (`foyer/src/hybrid/cache.rs:660-718`), so we replicate its
semantics minus the broken inflight layer (with one deliberate divergence on the
`Load::Piece` arm, noted below):

```text
get(key, expected_len):
  1. memory().get(key)                     // plain RAM lookup, no inflight (cache.rs:763)
       hit -> return bytes                 // caller re-validates length as today
  2. RAM miss -> per-key single-flight (see 3) whose task does:
       storage().load(key)                 // direct disk read, no inflight (store.rs:160)
         Load::Entry { key: _, value, populated } ->   // bind `populated`, read its pub `age`
             value.bytes.len() == expected_len ?       // field (`Populated` itself is not
               yes -> emit disk-age metric (value.disk_write_ms != DISK_WRITE_NONE);  // importable, see checklist 2)
                      promote: memory().insert_with_properties(key,
                          CachedBlock { bytes, ram_inserted_at: now,
                                        disk_write_ms: value.disk_write_ms,
                                        is_prefetch: false },
                          HybridCacheProperties::default().with_age(populated.age));  // parity with hybrid promotion
                      return bytes
               no  -> imetric range_cache_promotion_len_mismatch; return None (miss)
         Load::Piece { piece, populated } ->  // keeper/write-buffer hit, incl. prefetch phantoms
             read the value via piece.value() (a Piece<K,V,P>, not a CachedBlock);
             same validation; promote a *fresh normalized* CachedBlock (is_prefetch=false,
             ram_inserted_at=now, keep piece value's disk_write_ms), also with
             .with_age(populated.age). NOTE this is a deliberate divergence, not parity:
             hybrid's own Piece arm IGNORES populated (destructures `populated: _`, with an
             upstream `// TODO(MrCroxx): Remove populated with piece?`) and re-inserts the
             piece with its original properties. Applying the age here is justified on its
             own merits: keeper hits are Age::Young (store.rs:172), and Young is what makes
             a later RAM eviction skip a disk re-write the keeper flush already performs
             (see the Age::Young fact below). No disk-age metric when disk_write_ms ==
             DISK_WRITE_NONE (it wasn't a disk read)
         Load::Miss | Load::Throttled -> return None   // hybrid parity: throttled -> None to caller
         Err(e) -> existing error path (metric + warn + None)
```

Notes:
- The `Source::Disk` check in today's `get` disappears; with two-step we know the source
  by construction (step 2 hit == disk/keeper read), so the
  `object_cache_disk_tier_read_age_ms` metric moves into the load task.
- Fresh-normalized promotion also fixes a pre-existing telemetry artifact: hybrid's
  Piece promotion reused the phantom record (`is_prefetch=true`,
  stale `ram_inserted_at`), silently excluding promoted prefetch blocks from RAM
  eviction telemetry.
- The short disk entry is left in place on mismatch (same as today): the heal's
  `put(Demand)` supersedes it, and until then other probes coalesce on the origin
  single-flight. Behavior converges identically to the current code.

### 3. Per-key single-flight for the disk load (coalescing parity)

Foyer's inflight table coalesced concurrent same-key disk reads; we keep that:

```rust
// FoyerBackend field
loads: Arc<Mutex<HashMap<String, Shared<BoxFuture<'static, Option<Bytes>>>>>>
```

- First caller for a key `tokio::spawn`s the load-validate-promote task (step 2) and
  stores a `Shared` future awaiting its `JoinHandle`; followers clone the `Shared`.
- The spawned task removes its own map entry when done (always runs to completion, so
  no stale entries even when every awaiting query is cancelled).
- A detached task is *safe* here precisely because of the promotion gate: its only write
  is validated, so it never needs cancelling — the exact property foyer's design needs
  the (broken) close flag for.
- `Bytes` is cheaply clonable, satisfying `Shared`'s `Output: Clone`. `futures` and
  `tokio` are already object-cache dependencies.

### 4. Write paths unchanged

- `put(Demand)` — `cache.insert(...)` as today. Its `emplace → take` cancellation
  becomes a permanent no-op because nothing creates inflight entries anymore.
- `put(Prefetch)` — `storage_writer().force().insert(...)` as today (disk-only, no
  inflight interaction).

### 5. Telemetry improvements folded in (near-free byproducts of the restructure)

The two-step read makes two useful signals available at essentially no cost, so
they ride along with this change rather than a separate pass. Both fit the existing
`object_cache_{ram,disk}_tier_*` / `range_cache_*` naming and the bounded-cardinality
`PropertySet` taxonomy in `metric_tags.rs`.

- **Tiered hit counters.** Today the tier of a hit is not cleanly captured:
  `range_cache_block_backend_hit` (`fetch.rs:188`) carries no tier dimension, and the
  only tier signal is the *indirect* presence of `object_cache_disk_tier_read_age_ms`
  on disk reads. The two-step `get` knows the tier **by construction** — a step-1 hit
  (`memory().get`) is a RAM hit, a step-2 hit (`store.load`) is a disk hit — with no
  `Source::Disk` sniff needed. Emit, from inside `FoyerBackend::get`: `object_cache_ram_tier_hit`
  on the step-1 hit and `object_cache_disk_tier_hit` on the step-2 promote — but **only
  for block-key gets** (the `blk:`-prefixed keys `probe_blocks` passes in). `size()`'s
  `meta:`-prefixed 8-byte lookups (`range_cache/mod.rs:204`) also flow through
  `FoyerBackend::get` and must be excluded from these counters; otherwise they'd mix
  into the miss-rate derivation below, which counts only block requests. With meta
  gets excluded, miss rate is derivable at the fetch layer as
  `range_cache_block_request − (ram_hit + disk_hit)`, giving a proper *aggregate*
  tiered hit-rate — the primary input to RAM-sizing decisions — that is unavailable
  today. These counters reuse the `EvictionTagTable`/`{prefix}` tagging for consistency
  with the other tier metrics, but — as already true of
  `object_cache_disk_tier_read_age_ms` today — `classify` is fed the storage-prefixed
  key (`blk:...`), which never starts with a content label, so `{prefix}` always
  resolves to `"other"`; no per-content-prefix hit-rate breakdown exists yet, only the
  aggregate. Fixing that classification gap is out of scope here.
- **Coalescing fan-in counter.** The new per-key single-flight (§3) replaces foyer's
  inflight coalescing; a `range_cache_load_coalesced` count, incremented each time a
  follower clones an in-flight `Shared` instead of spawning its own load, makes the
  coalescing observable (and would have made the original clobber race visible). Cheap
  and directly validates machinery this plan introduces.

These are strictly additive; they do not affect the correctness argument below.

Consciously deferred to their own issues (not this PR): disk→RAM promotion volume
(count/bytes) — [#1321](https://github.com/madesroches/micromegas/issues/1321); and
steady-state tier-occupancy gauges — [#1322](https://github.com/madesroches/micromegas/issues/1322).
Latency distributions and the eviction-reason breakdown already exist, so nothing was
filed for those.

## Why this is complete (proof sketch)

Enumerate every RAM-tier write after the change:

1. `put(Demand)` — canonical full bytes from an exact-range origin GET.
2. Validated promotion — bytes whose length equals the caller's `expected_len`.

Keys are write-once, so per key there is exactly one canonical full-length byte string;
(1) and (2) both carry it, hence **every possible overwrite ordering yields identical
bytes**. Short bytes can exist only on disk (undersized prefetch) and can never cross
into RAM. And since nothing calls `HybridCache::get`/`get_or_fetch` anymore, no inflight
leader is ever created: the buggy cancellation path is unreachable, zombie leaders
cannot exist, and our own detached load task is harmless by the same validation
argument. (The byte-identity claim leans on the documented write-once key invariant —
the same assumption the whole cache design already rests on, e.g. `size()`'s
"never invalidated" comment.)

## Why there is no performance penalty

| Path | Before (hybrid get) | After |
|---|---|---|
| RAM hit | inflight-table check + RAM lookup | plain RAM lookup (`memory().get`) |
| Disk hit | `store.load` + memory insert (promotion) + inflight bookkeeping/notify | `store.load` + length compare + memory insert |
| Full miss | RAM lookup + `store.load` + inflight bookkeeping | RAM lookup + `store.load` + one map lock |
| Concurrent same-key reads | coalesced by inflight table | coalesced by our single-flight |

Same I/O, same promotion, same coalescing; the added work is one `usize` comparison and
one small mutex-guarded map op per RAM miss, and the removed work is foyer's inflight
bookkeeping and oneshot signaling. Promotion (disk hit → RAM residency for demand
accesses) is fully preserved — the architectural requirement that blocked the earlier
two-step proposal.

## Implementation checklist

1. `src/backend.rs` — add `expected_len: u64` to `RangeCacheBackend::get`; document the
   promotion-gate contract.
2. `src/foyer_backend.rs` — two-step `get` + single-flight map + promotion helper;
   move the disk-age metric; add `range_cache_promotion_len_mismatch` metric; keep the
   error-path metric/log. Remove the now-unused `Source` import (its only use, the
   `Source::Disk` check, is gone). Import notes (verified against 0.22.3): `Populated` is
   **not** re-exported by the `foyer` facade (only by `foyer-storage`'s own prelude, which
   is not a direct dep) — bind the `populated` field without naming the type and read its
   pub `age` field, so no import is needed. `.with_age` is **not** an inherent
   `HybridCacheProperties` method; it comes from the `Properties` trait
   (`foyer_common::properties::Properties`), which neither `foyer` nor `foyer-memory`
   re-exports — add `foyer-common = "0.22"` to the workspace deps (`rust/Cargo.toml`,
   alphabetical) and `foyer-common = { workspace = true, optional = true }` to
   `object-cache/Cargo.toml` with the `foyer` feature becoming
   `foyer = ["dep:foyer", "dep:foyer-common"]`, then `use foyer_common::properties::Properties;`.
   Also emit the §5 telemetry: `object_cache_ram_tier_hit` /
   `object_cache_disk_tier_hit`, for block-key (`blk:`-prefixed) gets only — excluding
   `size()`'s `meta:`-prefixed gets so the miss-rate derivation stays valid —
   `{prefix}`-tagged via the `EvictionTagTable` for consistency (though, per §5, this
   currently always resolves to `"other"`), and `range_cache_load_coalesced`
   (single-flight follower count).
3. `src/memory_backend.rs`, `src/bounded_memory_backend.rs` — accept/ignore the new
   parameter.
4. `src/range_cache/fetch.rs` — lift the `block_byte_range`/`expected_len` computation
   into the probe closure ahead of the `backend.get` call and pass it in; keep the
   caller-side length check (currently after the get) as defense in depth.
5. `src/range_cache/mod.rs` — `size()` passes `8`.
6. Tests — the `RangeCacheBackend::get` signature change breaks 14 single-arg call sites
   across `tests/foyer_backend_tests.rs` (6), `tests/range_cache_tests.rs` (3, lines
   267/840/947), and `tests/l1_store_tests.rs` (5, lines 25/31/43/60/88); update all
   three. New/updated tests in `tests/foyer_backend_tests.rs`:
   - **Short block never promoted**: `put(Prefetch)` undersized bytes → flush →
     `get(key, full_len)` returns `None` and `ram_usage()` unchanged; then `put(Demand)`
     full bytes → `get` returns them.
   - **Promotion works**: `put(Prefetch)` full block → `get` returns it and RAM usage
     grows (promotion observable); second `get` is a RAM hit.
   - **Coalescing**: N concurrent `get`s on a cold disk-resident key → `disk_stats()`
     read-IO delta is 1 (and, if metric readback is available in the harness,
     `range_cache_load_coalesced` == N-1).
   - **Heal survives concurrent readers** (regression for the original clobber): seed a
     short block on disk, run concurrent `get` loops while `put(Demand)` heals, assert
     the final `get` returns the healed bytes (deterministic now: no writer exists that
     can produce non-canonical RAM contents).
7. `mkdocs/docs/admin/object-cache.md` (Monitoring section) — document the new
   operator-facing signals: `object_cache_ram_tier_hit` / `object_cache_disk_tier_hit`
   (block-key gets only) and the miss-rate derivation from §5,
   `range_cache_load_coalesced`, and `range_cache_promotion_len_mismatch`.
8. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test -p
   micromegas-object-cache`, then full CI script.
9. Update the micromegas issue; file the upstream foyer bug (fix diff above) so the
   ecosystem gets a real fix eventually. When a fixed foyer releases, **keep this path —
   do not revert to `HybridCache::get`**. Reverting would re-inherit foyer's inflight
   behavior while discarding the length-gated promotion invariant, which is
   defense-in-depth worth having independent of foyer's correctness (the benignity of
   any future race would again rest on the emergent write-once/validation invariants
   instead of a local, explicit gate). The two-step path is also simpler and strictly
   less machinery than the inflight route; the pinned-composition coupling to foyer's
   promotion semantics (see the Age::Young fact above) is the one ongoing cost, and it
   is already paid.

## Verified API facts this design rests on (foyer 0.22.3 sources)

- `HybridCache::memory()` / `storage()` are public (`foyer/src/hybrid/cache.rs:491,496`).
- Memory `Cache::get` is a plain sync lookup, no inflight (`foyer-memory/src/cache.rs:763`);
  `insert_with_properties` (`:724`) allows `HybridCacheProperties::default().with_age(age)`
  parity with hybrid's own promotion; `remove` exists (`:748`) if ever needed.
- `Store::load(&key) -> Result<Load<K,V,P>>` hashes internally, checks the keeper
  (write buffer) then the engine, and self-verifies key equivalence
  (`foyer-storage/src/store.rs:160`); variants `Entry`/`Piece`/`Miss`/`Throttled`.
- `.with_age` exists only on the `Properties` trait
  (`foyer-common/src/properties.rs:103`, implemented for `HybridCacheProperties` at
  `foyer/src/hybrid/cache.rs:166`); the trait is re-exported by neither `foyer` nor
  `foyer-memory`, hence the `foyer-common` dep in checklist 2. `Populated` is likewise
  absent from the `foyer` facade's prelude; its `age` field is pub, so binding the
  struct suffices.
- Age handling is load-bearing, not cosmetic: the block engine's `enqueue` **skips the
  disk write** for `Age::Young` pieces (`foyer-storage/src/engine/block/engine.rs:582-590`).
  Promoting with default properties (`Age::Fresh`) would re-write every promoted block
  to disk on its next RAM eviction — write amplification the "no performance penalty"
  section rules out. (Disk-engine `Entry` hits carry `Age::Old` or `Age::Young`
  depending on probation state, `engine.rs:704-707`; keeper hits are always
  `Age::Young`, `store.rs:172`. Passing `populated.age` through covers both.)
- `HybridCache::get` is exactly memory-get-or-fetch over a `store.load` closure mapping
  `Entry→promote with .with_age(populated.age)`, `Piece→re-insert the piece itself
  (populated ignored — upstream TODO)`, `Throttled|Miss→None`
  (`foyer/src/hybrid/cache.rs:660-718`) — the two-step read replicates the Entry arm
  1:1 minus the inflight layer, and deliberately diverges on the Piece arm
  (fresh-normalized value + `.with_age(populated.age)`, see the §2 notes).
- The fetch leader is a detached spawned task (`foyer-memory/src/raw.rs:1034`), which is
  why *no* lock/ordering scheme at our layer can bound it — validation, not
  cancellation, is the workable invariant.
- `object-cache/Cargo.toml` already depends on `futures` and `tokio`.

## Rejected alternatives (kept for the record)

1. **Git fork + `[patch.crates-io]`** — complete, but requires an external fork to
   maintain and a `deny.toml` `[sources]` policy change (git deps are deliberately
   denied).
2. **Vendor + local patch** — complete and self-contained, but carries a vendored
   third-party tree plus fmt/clippy/machete/CI carve-outs.
3. **Accept-as-benign (no code change)** — defensible on severity (no consumer can serve
   clobbered bytes) but leaves a known race and wasted refetches in place; superseded by
   this design, which removes the race outright for comparable effort.
4. **Two-step read *without* length-gated promotion** — relocates the identical clobber
   into our promotion insert; the validation gate is what turns it from "same severity"
   into "provably harmless".
5. **Per-key lock serializing `get`/`put(Demand)`** — unsound: cancelled `get` futures
   leave foyer's detached zombie leader running past any lock scope we control.
