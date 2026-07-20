# foyer-memory Decoy Close-Flag Fix Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1318

## Overview

`foyer-memory` 0.22.3 (the current release; no newer version is published) has a bug in
`InflightManager::enqueue`'s `Entry::Vacant` arm (`src/inflight.rs`): the fetch leader is handed a
*different* `Arc<AtomicBool>` "close" flag from the one stored in the inflight table. Callers that
cancel an inflight fetch (`take`, `fetch_or_take`) set the table's copy; the leader's `RawFetch::poll`
checks its own (never-set) copy, so cancellation is inert. In `object-cache`'s hybrid cache
(`FoyerBackend`, `rust/object-cache/src/foyer_backend.rs`), this means an `insert()` (`put()` with
`FillHint::Demand`) racing an in-flight disk load of the same key does not actually cancel that load:
the load completes, decides its result is still current, and **re-inserts the stale disk value over
the newer insert** — silent data-correctness regression under heavy disk-tier promotion traffic.

This plan vendors a minimally patched copy of `foyer-memory` 0.22.3 into the repo, applies the
one-line fix (share a single `Arc` between the table entry and the leader) suggested in the issue,
adds a regression test that exercises the exact bug (and would fail without the fix), and wires the
patched copy in via `[patch.crates-io]` so every consumer of `foyer-memory` (both `object-cache`'s
direct dependency and `foyer`'s transitive one, which is what `HybridCache` actually uses) gets the
fix. No behavior in `object-cache` itself changes; this is purely a dependency-level bug fix.

## Current State

### The bug (`foyer-memory-0.22.3/src/inflight.rs`, `InflightManager::enqueue`, lines 206-228)

```rust
Entry::Vacant(v) => {
    let (tx, rx) = oneshot::channel();
    let id = self.next_id;
    self.next_id += 1;
    let entry = InflightEntry {
        hash,
        key: key.to_owned(),
        inflight: Inflight {
            id,
            close: Arc::new(AtomicBool::new(false)),   // Arc A -- stored in the table
            notifiers: vec![tx],
            f: None,
        },
    };
    v.insert(entry);
    let close = Arc::new(AtomicBool::new(false));       // Arc B -- a different Arc
    Enqueue::Lead {
        id,
        close,                                          // leader gets Arc B
        waiter: rx.into_future(),
        required_fetch_builder: f,
    }
}
```

`InflightManager::take` (called from `raw.rs::emplace`, the direct-`insert()` path, at
`take(record.hash(), record.key(), None)`) and `fetch_or_take` both call
`inflight.close.store(true, Ordering::Relaxed)` on the **table's** `Inflight` — Arc A. But the fetch
leader's `RawFetch::poll` (in the `FetchOptional`/`FetchRequired` states) checks `this.close.load(...)`
on **its own** copy — Arc B, which nothing ever writes to. The cancellation signal never reaches the
leader.

### Where this manifests in `object-cache`

`FoyerBackend` (`rust/object-cache/src/foyer_backend.rs:355-393`) has two write hints:
- `FillHint::Demand` → `self.cache.insert(key, CachedBlock::new(owned))` (line 390) — a direct
  `HybridCache::insert`, which internally reaches `raw.rs::emplace` → `InflightManager::take`.
- `get()` (line 318) → `self.cache.get(key).await`, which on a RAM miss triggers a disk-tier fetch
  through the inflight-coalescing path (`InflightManager::enqueue`) so concurrent `get()`s for the
  same key share one disk read.

When a `put()` (demand fill, e.g. a fresh origin read) lands for a key while a `get()`'s disk load for
that same key is still in flight, `emplace` removes the inflight entry and requests cancellation —
but per the bug above, the in-flight leader never observes it, finishes its disk read, and reinserts
the stale value, clobbering the fresher one. This is exactly the promotion-vs-fill race the object-cache
disk tier is built to serve under load (`round_trip_through_disk_tier` in
`rust/object-cache/tests/foyer_backend_tests.rs` already exercises the same disk-promotion path
without racing it).

### Dependency wiring today

- `rust/Cargo.toml` (workspace root): `foyer = "0.22"` and `foyer-memory = "0.22"` as plain
  crates-io dependencies, no `[patch]` section.
- `rust/object-cache/Cargo.toml:24`: `foyer-memory.workspace = true` (direct use: `BoundedMemoryBackend`
  (`rust/object-cache/src/bounded_memory_backend.rs`) builds and drives a standalone
  `foyer_memory::Cache` via `Cache`/`CacheBuilder`/`LfuConfig`, calling only plain `insert`/`get` — it
  never goes through a fetch/inflight-coalescing API, so this path is unaffected by the bug even
  though it depends directly on `foyer-memory`'s internals).
- `Cargo.lock` resolves `foyer`, `foyer-memory`, `foyer-common`, `foyer-storage`, `foyer-tokio` all
  at `0.22.3` from crates.io.
- `rust/Cargo.toml`'s workspace `members = ["*", "examples/write-perfetto"]` with
  `exclude = ["target", ".claude", ".cargo", ".*", "datafusion-wasm", "examples"]` — any top-level
  directory under `rust/` with a `Cargo.toml` is auto-included as a member unless excluded.
  `datafusion-wasm` is the existing precedent for a excluded, separately-built sub-tree with its own
  CI steps (`build/rust_ci.py`'s `run_wasm()`).
- `rust/deny.toml`'s `[sources]` only restricts `unknown-registry`/`unknown-git` (allowing only
  `crates.io`); a local `path` patch is neither, so it is unaffected by that check. License checks
  read the resolved crate's own `Cargo.toml` `license` field, which stays `"Apache-2.0"` (already
  allowed in `[licenses] allow`).
- `InflightManager`, `Enqueue`, `Inflight`, `InflightEntry`, `FetchOrTake` are **not** re-exported
  from `foyer_memory`'s public API (`src/lib.rs` has `mod inflight;`, not `pub mod`; `src/prelude.rs`
  only re-exports `FetchTarget`, `Notifier`, `OptionalFetch*`, `RequiredFetch*`, `Waiter`) — so a
  regression test that calls `InflightManager::enqueue`/`take` directly can only live *inside* the
  `foyer-memory` crate itself, not in `object-cache`.

## Design

### Fix

Vendor `foyer-memory` 0.22.3 verbatim into `rust/vendor/foyer-memory-0.22.3/` and apply the upstream-suggested
fix to `src/inflight.rs`'s `Entry::Vacant` arm — store one `Arc`, clone it into both places:

```rust
Entry::Vacant(v) => {
    let (tx, rx) = oneshot::channel();
    let id = self.next_id;
    self.next_id += 1;
    let close = Arc::new(AtomicBool::new(false));
    let entry = InflightEntry {
        hash,
        key: key.to_owned(),
        inflight: Inflight {
            id,
            close: close.clone(),
            notifiers: vec![tx],
            f: None,
        },
    };
    v.insert(entry);
    Enqueue::Lead {
        id,
        close,
        waiter: rx.into_future(),
        required_fetch_builder: f,
    }
}
```

Wire it in via a path patch in `rust/Cargo.toml`:

```toml
[patch.crates-io]
foyer-memory = { path = "vendor/foyer-memory-0.22.3" }
```

`[patch.crates-io]` overrides *every* resolution of the `foyer-memory` crate from crates.io across
the whole dependency graph — including `foyer`'s own transitive dependency on it (what `HybridCache`
actually uses) and `object-cache`'s direct one — with a single patch point. No changes are needed to
`foyer-memory`'s advertised version (`0.22.3`, kept identical in the vendored `Cargo.toml`) or to the
`foyer = "0.22"` / `foyer-memory = "0.22"` version requirements in `rust/Cargo.toml`.

### Why vendor-and-patch, not a git fork

See Trade-offs below — short version: no external GitHub state (fork, push) is required, the change
is fully contained in this repository, trivially auditable (one function changed against an otherwise
verbatim copy), and trivially removable once upstream ships a real fixed release.

### Keeping the vendored copy out of workspace-wide checks

Add `"vendor"` to `rust/Cargo.toml`'s workspace `exclude` list (alongside the existing
`datafusion-wasm` precedent), so:
- `cargo fmt --check` and `cargo clippy --workspace -- -D warnings` do not sweep over third-party
  code we did not write and want to stay diff-minimal against upstream.
- `cargo test` (workspace-default) does not pick it up as a member.

Workspace `exclude` only affects Cargo-workspace-membership-based tooling (fmt/clippy/test); it does
*not* affect `cargo machete`, which walks the filesystem directly, respecting `.gitignore`/`.ignore`
files rather than `Cargo.toml` workspace membership. `build/rust_ci.py`'s existing "Unused
Dependencies Check" step invokes bare `cargo machete` with no path restriction, so without a separate
opt-out it would still walk into `rust/vendor/foyer-memory-0.22.3/Cargo.toml` and lint its (large,
untouched) third-party dependency list. Add `rust/vendor/.ignore` containing `*` (or a `rust/.ignore`
listing `vendor/`) so ignore-aware tools like `cargo machete` skip the vendored tree too.

This means the vendored crate's own test suite (upstream's existing tests, plus the new regression
test below) needs its own CI step, mirroring the existing `datafusion-wasm` pattern in
`build/rust_ci.py` (a dedicated step with `cwd` pointed at the sub-tree).

### Regression test

Because `InflightManager` and `Enqueue` are private to `foyer_memory` (not part of its public API),
the regression test is added directly inside the vendored crate, in `src/inflight.rs`'s own
`#[cfg(test)] mod tests`, reusing the same test scaffolding (`Fifo<u64, u64, TestProperties>`,
`ModHasher`, `HashTableIndexer`) already used by `src/raw.rs`'s test module:

```rust
#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use foyer_common::hasher::ModHasher;

    use super::*;
    use crate::eviction::{fifo::Fifo, test_utils::TestProperties};
    use crate::indexer::hash_table::HashTableIndexer;

    type TestInflightManager =
        InflightManager<Fifo<u64, u64, TestProperties>, ModHasher, HashTableIndexer<Fifo<u64, u64, TestProperties>>>;

    #[test]
    fn leader_close_flag_observes_concurrent_take() {
        let mut manager = TestInflightManager::new();
        let hash = 1;
        let key = 42u64;

        let close = match manager.enqueue::<u64, ()>(hash, &key, None) {
            Enqueue::Lead { close, .. } => close,
            Enqueue::Wait(_) => panic!("expected Lead for a vacant key"),
        };

        // Mirrors raw.rs::emplace's direct-insert path: `take(hash, key, None)`
        // removes the inflight entry and requests cancellation.
        manager.take::<u64>(hash, &key, None);

        assert!(
            close.load(Ordering::Relaxed),
            "leader's close flag must observe the table's cancellation (issue #1318)"
        );
    }
}
```

This reproduces the exact call sequence from `raw.rs::emplace` (`take(hash, key, None)`) that
triggers the bug in production, fails before the fix (leader's `Arc` is never written to) and passes
after it, with no timing dependence.

## Implementation Steps

1. **Vendor the crate.** Copy `foyer-memory` 0.22.3's published source tree (from
   `~/.cargo/registry/src/*/foyer-memory-0.22.3/`, the normalized/published `Cargo.toml`, not
   `Cargo.toml.orig`) into `rust/vendor/foyer-memory-0.22.3/`:
   - `src/` (all of `cache.rs`, `eviction/`, `indexer/`, `inflight.rs`, `pipe.rs`, `raw.rs`,
     `record.rs`, `lib.rs`, `prelude.rs`, `test_utils.rs`) verbatim.
   - `Cargo.toml`, trimmed: drop the `[[bench]]` entries and the `benches/`-only dev-dependencies
     (`csv`, `moka`, `rand_distr`) since benches are not vendored; keep `rand`, `futures-util`,
     `test-log` (used by `src/raw.rs`'s existing test module).
   - Add a short header note (top of `Cargo.toml` or a new `VENDOR_NOTES.md`) recording: this is a
     vendored, patched copy of `foyer-memory` 0.22.3 (https://crates.io/crates/foyer-memory),
     Copyright the foyer Project Authors, Apache-2.0, patched for
     https://github.com/madesroches/micromegas/issues/1318 pending an upstream release; remove this
     directory and the `[patch.crates-io]` entry once a `foyer-memory` release ships with the fix.
   - Add `LICENSE-APACHE` (standard Apache License 2.0 text) alongside it — the published crate
     tarball omits a bundled `LICENSE` file (it lives at the `foyer-rs/foyer` monorepo root), but
     redistributing a modified copy requires including the license text.
2. **Apply the fix** to `rust/vendor/foyer-memory-0.22.3/src/inflight.rs`'s `enqueue` `Entry::Vacant`
   arm as shown in Design above.
3. **Add the regression test** to the same file's new `#[cfg(test)] mod tests`, as shown above.
4. **Wire the patch** in `rust/Cargo.toml`:
   - Add `"vendor"` to the workspace `exclude` list.
   - Add the `[patch.crates-io]` section pointing `foyer-memory` at
     `vendor/foyer-memory-0.22.3`.
   - Add `rust/vendor/.ignore` (containing `*`) so ignore-aware tools such as `cargo machete` also
     skip the vendored tree (workspace `exclude` alone does not affect `cargo machete`).
5. **Add a CI step** in `build/rust_ci.py`'s `run_native()` (or a new small `run_vendor()` following
   the `run_wasm()` precedent) that runs `cargo test` with `cwd` set to
   `rust/vendor/foyer-memory-0.22.3`, so the vendored crate's test suite (including the new
   regression test) runs every CI pass.
6. **Rebuild and refresh the lockfile**: run `cargo build` (or `cargo check`) from `rust/` so
   `Cargo.lock` records the patched path source for `foyer-memory`, and commit the updated lockfile.
7. **Verify no CI regressions locally** before committing:
   - `cargo fmt --check` and `cargo clippy --workspace -- -D warnings` (should not touch `vendor/`).
   - `cargo test` (workspace) — unaffected, still excludes `vendor/`.
   - `cargo machete` — confirm `rust/vendor/.ignore` keeps it from scanning
     `rust/vendor/foyer-memory-0.22.3/Cargo.toml`.
   - `cargo test` inside `rust/vendor/foyer-memory-0.22.3` — the new regression test passes; on a
     scratch copy without the fix it fails, confirming the test actually exercises the bug.
   - `cargo deny check licenses bans sources` and `cargo audit` — confirm the patched path source
     does not trip the `[sources]` unknown-registry/unknown-git checks or license checks.
8. **Manual follow-up (not part of this PR):** file an upstream bug report against
   `foyer-rs/foyer` describing the decoy-Arc bug (the issue body already contains the suggested
   fix diff verbatim, ready to paste). This is a public action against a third-party repository and
   is out of scope for this repo's automated implementation; do it separately once this fix lands.

## Files to Modify

- `rust/Cargo.toml` — add `"vendor"` to workspace `exclude`; add `[patch.crates-io]` section.
- `rust/Cargo.lock` — regenerated (patched `foyer-memory` source).
- `rust/vendor/.ignore` — new (`*`, so `cargo machete` and other ignore-aware tools skip the
  vendored tree).
- `rust/vendor/foyer-memory-0.22.3/Cargo.toml` — new (trimmed, published metadata).
- `rust/vendor/foyer-memory-0.22.3/src/**` — new (vendored source; `inflight.rs` patched + tested).
- `rust/vendor/foyer-memory-0.22.3/LICENSE-APACHE` — new.
- `rust/vendor/foyer-memory-0.22.3/VENDOR_NOTES.md` — new (provenance/removal note).
- `build/rust_ci.py` — new CI step testing the vendored crate.

## Trade-offs

- **Vendor + local patch (chosen)** vs. **fork `foyer-rs/foyer` on GitHub + git dependency**: a git
  fork would require creating and pushing to an external repository (a state-changing action on a
  third-party service, outside this repo's control and outside what an automated implementation
  should do unprompted) and adds an external moving part (the fork's branch) for a one-line fix. The
  vendored copy is fully self-contained, auditable as a single small diff against a verbatim upstream
  copy, and trivially removable (delete `vendor/foyer-memory-0.22.3`, drop the `[patch]` entry) once
  `foyer-rs/foyer` ships a real release with the fix.
- **Vendor + local patch** vs. **work around the race in `object-cache`'s own call sites** (e.g.
  avoid direct `cache.insert` while a fetch might be in flight): rejected — the race is intrinsic to
  normal `object-cache` usage (a fresh origin fill racing a disk-tier promotion read of the same
  key), so there is no call-site-level workaround that doesn't itself change core caching behavior;
  it also would not fix the bug for any other consumer of `foyer-memory`'s inflight-coalescing.
- **Excluding `vendor/` from the workspace** (chosen) vs. **making it a workspace member**: as a
  member, `cargo test --workspace` would pick up its tests automatically with no `rust_ci.py`
  change, but `cargo fmt --check` and `cargo clippy --workspace -- -D warnings` would then also apply
  to third-party code we do not intend to restyle or clean up, and any future `cargo fmt` run in this
  repo would silently reformat the vendored copy away from something diffable against upstream.
  Excluding it (matching the existing `datafusion-wasm` precedent) keeps the vendored tree verbatim
  except for the one patched function, at the cost of one extra explicit CI step.

## Testing Strategy

- New unit test `leader_close_flag_observes_concurrent_take` in
  `rust/vendor/foyer-memory-0.22.3/src/inflight.rs`, run via the new CI step
  (`cargo test` with `cwd` in the vendored crate directory). Verify manually during implementation
  that this test fails against an unpatched copy of `inflight.rs` (confirms it actually exercises the
  bug) and passes after the fix.
- Existing `rust/object-cache/tests/foyer_backend_tests.rs::round_trip_through_disk_tier` continues
  to pass unchanged (sanity check that the patched crate is otherwise behaviorally identical for
  object-cache's disk-tier round trip).
- Full `python3 build/rust_ci.py` run (formatting, clippy, machete, audit, deny, tests, plus the new
  vendored-crate test step) to confirm no regressions from the patch/vendoring/exclude changes.

## Open Questions

- None blocking — the one external follow-up (filing the upstream `foyer-rs/foyer` issue) is called
  out above as an explicit manual step outside this PR's scope, not a design ambiguity.
