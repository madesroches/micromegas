# foyer-memory Decoy Close-Flag — Fix Design (validated-promotion two-step read)

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1318

> **Status (2026-07-20): solution designed, ready to implement. No code changed yet.**
> The fix is entirely in object-cache (no vendoring, no fork, no `[patch]`): replace
> `HybridCache::get` with a two-step read (RAM lookup, then direct disk load) whose
> disk→RAM **promotion is gated on a caller-supplied expected length**, plus our own
> per-key single-flight around the disk load to keep read-coalescing parity. This makes
> foyer's buggy inflight path **unreachable** and makes every remaining write race
> **provably byte-identical**, at no performance cost.

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
  `block_byte_range` today (it currently validates *after* the get; it keeps that check
  and additionally passes the value down).
- `size()` (`range_cache/mod.rs:204`) passes `8`.

`MemoryBackend` / `BoundedMemoryBackend` (single-tier, no promotion) accept and ignore
the parameter.

### 2. `FoyerBackend::get` becomes a two-step read — no foyer inflight path

Replace `self.cache.get(key).await` (which routes through `memory.get_or_fetch_inner` →
`InflightManager::enqueue`, creating the buggy leader) with the public non-inflight
APIs. Verified against foyer 0.22.3 sources — `HybridCache::get` is *exactly* this
composition internally (`foyer/src/hybrid/cache.rs:661-719`), so we replicate its
semantics minus the broken inflight layer:

```text
get(key, expected_len):
  1. memory().get(key)                     // plain RAM lookup, no inflight (cache.rs:763)
       hit -> return bytes                 // caller re-validates length as today
  2. RAM miss -> per-key single-flight (see 3) whose task does:
       storage().load(key)                 // direct disk read, no inflight (store.rs:160)
         Load::Entry { value, populated: { age } } ->
             value.bytes.len() == expected_len ?
               yes -> emit disk-age metric (value.disk_write_ms != DISK_WRITE_NONE);
                      promote: memory().insert_with_properties(key,
                          CachedBlock { bytes, ram_inserted_at: now,
                                        disk_write_ms: value.disk_write_ms,
                                        is_prefetch: false },
                          HybridCacheProperties::default().with_age(age));  // parity with hybrid promotion
                      return bytes
               no  -> imetric range_cache_promotion_len_mismatch; return None (miss)
         Load::Piece { piece, .. } ->      // keeper/write-buffer hit, incl. prefetch phantoms
             same validation; promote a *fresh normalized* CachedBlock (is_prefetch=false,
             ram_inserted_at=now, keep piece value's disk_write_ms); no disk-age metric
             when disk_write_ms == DISK_WRITE_NONE (it wasn't a disk read)
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
   error-path metric/log.
3. `src/memory_backend.rs`, `src/bounded_memory_backend.rs` — accept/ignore the new
   parameter.
4. `src/range_cache/fetch.rs` — pass `expected_len` (already computed) into
   `backend.get`; keep the caller-side length check as defense in depth.
5. `src/range_cache/mod.rs` — `size()` passes `8`.
6. Tests (`tests/foyer_backend_tests.rs`, plus trait-signature updates elsewhere):
   - **Short block never promoted**: `put(Prefetch)` undersized bytes → flush →
     `get(key, full_len)` returns `None` and `ram_usage()` unchanged; then `put(Demand)`
     full bytes → `get` returns them.
   - **Promotion works**: `put(Prefetch)` full block → `get` returns it and RAM usage
     grows (promotion observable); second `get` is a RAM hit.
   - **Coalescing**: N concurrent `get`s on a cold disk-resident key → `disk_stats()`
     read-IO delta is 1.
   - **Heal survives concurrent readers** (regression for the original clobber): seed a
     short block on disk, run concurrent `get` loops while `put(Demand)` heals, assert
     the final `get` returns the healed bytes (deterministic now: no writer exists that
     can produce non-canonical RAM contents).
7. `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test -p
   micromegas-object-cache`, then full CI script.
8. Update the micromegas issue; file the upstream foyer bug (fix diff above) so the
   ecosystem gets a real fix eventually. When a fixed foyer releases, we may keep this
   path (it is simpler and strictly less machinery than the inflight route) or revert to
   `HybridCache::get` — either works; no urgency.

## Verified API facts this design rests on (foyer 0.22.3 sources)

- `HybridCache::memory()` / `storage()` are public (`foyer/src/hybrid/cache.rs:491,496`).
- Memory `Cache::get` is a plain sync lookup, no inflight (`foyer-memory/src/cache.rs:763`);
  `insert_with_properties` (`:724`) allows `HybridCacheProperties::default().with_age(age)`
  parity with hybrid's own promotion; `remove` exists (`:748`) if ever needed.
- `Store::load(&key) -> Result<Load<K,V,P>>` hashes internally, checks the keeper
  (write buffer) then the engine, and self-verifies key equivalence
  (`foyer-storage/src/store.rs:160`); variants `Entry`/`Piece`/`Miss`/`Throttled`.
- `HybridCache::get` is exactly memory-get-or-fetch over a `store.load` closure mapping
  `Entry→promote (with age)`, `Piece→promote`, `Throttled|Miss→None`
  (`foyer/src/hybrid/cache.rs:661-719`) — the two-step read replicates it 1:1 minus the
  inflight layer.
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
