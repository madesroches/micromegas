# Static Property Values Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1655

## Overview

The log and metric macros intern a callsite's properties once only when every value is a
string literal. A value chosen at runtime goes through the dynamic path, even when it is a
`&'static str` from a small closed set, because the macro can't see its lifetime. That path
costs about 4× an interned set, so hot paths precompute a `PropertySet` per label combination
and spread it with `..set` (`rust/object-cache/src/metric_tags.rs`). This plan adds a
`key = static expr` form. Each callsite caches the interned set for each distinct tuple of its
static values in an `scc::HashIndex`, so a call costs a few ns more than `..set` while the
property names stay at the call site. `metric_tags.rs` then shrinks to label resolution. No SQL,
wire format or `SCHEMA_VERSION` change: the events and sets emitted are the ones `..set`
produces today.

## Current State

**Macro muncher** (`rust/tracing/src/macros.rs:508`, `__props!`). State is
`$kind $ctx [$s] [$d] [$a] [$set]`:

- `$s`: literal pairs `($k, (value_tokens))`, interned once per callsite by `__callsite_set!`
  (`:701`) through `property_set::callsite_set` (`rust/tracing/src/property_set.rs:50`), which
  resolves duplicate names case-insensitively, last wins.
- `$d`: non-literal pairs tagged `bare` / `disp` / `dbg`, turned into a `DynProperty` slice by
  `__dyn_slice!` (`:714`).
- `$a`: every pair in source order, literals tagged `lit`. Used only when a `..set` is spread:
  the literals then go to the dynamic slice and are merged over the set.
- `$set`: the `..set` expression, if any.
- Value arms are ordered negative literal, `-` fallback, `%`, `?`, literal, then the bare `@e`
  fallback (`:524-560`). `@end` arms (`:570-610`) pick the cheapest call: `plain`, interned set
  only, or set plus dynamic slice. Log arms with dynamic values use the `gate` form so values
  are evaluated only for an enabled entry (`:644`); the set expression is evaluated inside the
  level check in both `nogate` and `gate`.

**Dispatch** (`rust/tracing/src/dispatch.rs`): `tagged_*_metric_dyn` and `log_dyn_tagged*` fall
back to the interned-set path when the dynamic slice is empty. Otherwise `merge_properties`
(`rust/tracing/src/dyn_deps.rs:63`) builds and sorts a `Vec`, and `DynDeps::intern_set` hashes
and probes it under the stream lock. Static values from the base set are referenced by pointer
(#1679).

**Duplicate-key rules** (pinned by `duplicate_keys`, `rust/tracing/tests/property_macro_tests.rs:412`):
without `..set`, among literals last wins case-insensitively and a dynamic value beats a literal
of the same name regardless of order; with `..set`, all pairs resolve in source order (a later
literal beats an earlier dynamic value of the same name); pairs override `..set` entries.

**wasm32** (`rust/tracing/src/lib.rs:170`): `property_set` is a stub where only the empty set
exists; `callsite_set` returns it. `internment`, `rustc-hash` and the other property deps are
native-only (`rust/tracing/Cargo.toml`).

**Precomputed sets in object-cache** (`rust/object-cache/src/metric_tags.rs`):

- `PrefixTags { label, prefix, prefix_demand, prefix_prefetch }` and
  `EvictionTags { prefix, count_evict, count_replace, count_remove, count_clear }` hold one set per
  combination, built at construction.
- Spread sites: `foyer_backend.rs:230,240,416,422,426,439,541`,
  `range_cache/fetch.rs:164,197,419,425` and `range_cache/mod.rs:248,251,277,298` (the last four
  spread `prefix_tag = self.classify_tags(key).prefix`, `:241`); `fetch.rs:85-87,318,384` pick
  and thread the class set through a helper, and `probe_blocks` (`fetch.rs:144`) and the
  function at `fetch.rs:267-268` carry sets as `&'static PropertySet` parameters;
  `range_cache/mod.rs:95-177` owns the `PrefixTags` table.
  `rust/object-cache/tests/metric_tags_tests.rs` imports `PrefixTags` and calls `for_class`.

## Design

### Syntax

```rust
imetric!("object_cache_promotion_count", "count", prefix = static t.label, 1_u64);

imetric!(
    "object_cache_ram_tier_eviction_count", "count",
    prefix = static t.label,
    reason = static reason_str(reason),
    tier = "ram",
    1_u64
);

info!(prefix = static t.label, request = %id, "fetched");
```

- `static expr` must be a `&'static str`. The macro passes it through
  `property_set::static_value(v: &'static str) -> &'static str`, so a borrowed value is a
  lifetime error at the call site rather than a silent bug.
- Keys go through `__prop_key!`, so reserved names are rejected as for the other forms.
- Mixes freely with literals, dynamic values and a trailing `..set`.

### Muncher changes

- New value arm `$k:tt = static $v:expr`, placed before the negative-literal arm (it must
  precede `@e`, and `static` never starts a literal).
- `$s` entries gain a tag: `($k, lit, (tokens))` for literals and `($k, stat, ($v))` for
  static values, both in source order. `__callsite_set!` matches only `lit` entries.
- New flag accumulator `$t`: `[]`, or `[static]` once a static value is seen. The `@end` arms
  match on it, so the existing expansions are unchanged when no static value is used.
- With `$t = [static]`, the base set expression becomes
  `__static_set!($kind [$set?] $($s)+)` and the dynamic slice is `$d` only. This also holds
  when `..set` is spread: literals and static values are interned over it in the cache, instead
  of going to the dynamic slice through `$a`.

The resulting `@end` matrix (static present):

| `..set` | dynamic values | base set | slice | log form |
|---|---|---|---|---|
| no | no | `__static_set!(s)` | empty | `gate` |
| no | yes | `__static_set!(s)` | `$d` | `gate` |
| yes | no | `__static_set!(set, s)` | empty | `gate` |
| yes | yes | `__static_set!(set, s)` | `$d` | `gate` |

With an empty slice, dispatch already takes the interned path, so no dispatch change is needed.
Every static row uses the `gate` log form: `nogate` evaluates the set expression before the
target-enabled check, so only `gate` guarantees a static expression is evaluated for an enabled
entry alone.

### `__static_set!` expansion

```rust
{
    static CACHE: $crate::static_set_cache::StaticSetCache<{ [$(stringify!($k)),+].len() }> =
        $crate::static_set_cache::StaticSetCache::new();
    CACHE.get(
        base,  // `$set` or `PropertySet::empty()`
        [$(( __prop_key!($kind $k), value )),+],  // value: `concat!(..)` or `static_value($v)`
    )
}
```

`M` is the number of `$s` entries, counted by the const block in the `static CACHE` type (a
static's type is never inferred).

### `StaticSetCache` (`rust/tracing/src/static_set_cache.rs`, new, native only)

```rust
type Key<const M: usize> = (usize, [(usize, usize); M]);  // base ptr, (ptr, len) per value

pub struct StaticSetCache<const M: usize> {
    map: scc::HashIndex<Key<M>, &'static PropertySet, FxBuildHasher>,
}

impl<const M: usize> StaticSetCache<M> {
    pub const fn new() -> Self;  // HashIndex::with_hasher(FxBuildHasher), const without scc's loom feature
    pub fn get(
        &self,
        base: &'static PropertySet,
        pairs: [(&'static str, &'static str); M],
    ) -> &'static PropertySet;
}
```

- **Key**: the base set's address plus `(ptr, len)` of every value. Names are fixed per callsite
  and stay out of the key. Literal values are in it because that keeps one ordered pair list
  (and so the source-order duplicate rule) without a second macro accumulator; they are constant,
  so they never add entries. Including `len` keeps two substrings sharing a start address apart.
- **Hit**: `peek_with(&key, |_, v| *v)`. `HashIndex` reads take no lock.
- **Miss**: build the properties from `base`, then apply `pairs` in order with case-insensitive
  last-wins, then `PropertySet::find_or_create`, then `insert_sync`. Two threads missing the same
  key both intern the same set; the losing insert is ignored. The pair-application loop is shared
  with `callsite_set` (extract it as `apply_pairs(&mut Vec<Property>, pairs)` in
  `property_set.rs`), so both forms resolve duplicates the same way.
- **Growth**: one entry per distinct key, never removed. Equal strings at different addresses get
  separate entries that point at the same interned set. Bounded by the same cardinality contract
  as `PropertySet`, which the docs state for `static` explicitly.

**wasm32**: add a stub `static_set_cache` module next to the `property_set` stub in `lib.rs`.
`StaticSetCache::get` returns `PropertySet::empty()`. Also add an identity
`pub fn static_value(v: &'static str) -> &'static str` to the `property_set` stub module, since the
macro calls `$crate::property_set::static_value`. Call sites compile unchanged and type-check the same.

### Precedence

Unchanged rules, extended: among literals and static values, last in source order wins
(case-insensitively); dynamic values beat both; any pair overrides a `..set` entry. A call with
both `static` and `..set` differs from one without `static`: literals and static values are
interned into the base set and dynamic values are merged over it, so dynamic values beat
literals and static values regardless of order.

### Dependencies

`scc` becomes a native-only normal dependency of `micromegas-tracing`, added to the workspace
`rust/Cargo.toml` (`scc = "3.8"`), and pulls in `sdd` and `saa`. Both are pure Rust with no
target-specific code; `scc` runs loom model tests.

## Implementation Steps

### Phase 1: Cache

1. `rust/Cargo.toml`: add `scc` to `[workspace.dependencies]`. `rust/tracing/Cargo.toml`: add
   `scc.workspace = true` under the native-only dependencies. Regenerate
   `rust/datafusion-wasm/Cargo.lock` (excluded from the workspace, audited in CI), which gains
   `scc`, `sdd` and `saa`.
2. `rust/tracing/src/property_set.rs`: extract `apply_pairs` from `callsite_set`; add
   `static_value`.
3. `rust/tracing/src/static_set_cache.rs`: `StaticSetCache` as above. Register it in
   `rust/tracing/src/lib.rs` under `cfg(not(wasm32))`, plus the wasm stub: a `static_set_cache` stub
   module and an identity `static_value` in the `property_set` stub module.

### Phase 2: Macros

4. `rust/tracing/src/macros.rs`: tag `$s` entries, add the `static` arm and the `$t` flag, add
   the four `@end` arms per kind, add `__static_set!`, update `__callsite_set!` to match `lit`
   entries.
5. Rustdoc on `imetric!` and `log!`: describe `static`, its `&'static str` requirement, the
   cardinality promise, and the precedence; add `compile_fail` doctests (see Testing).

### Phase 3: Benchmarks

6. `rust/tracing/benches/properties.rs`: add `metric/static_low_card` and `log/static_low_card`
   (a leaked label chosen per call, like `dyn_low_card`) and `metric/set_low_card`
   (precomputed sets for the same labels), so the static form is tracked against both
   alternatives.
7. Delete the prototype `rust/tracing/benches/static_values.rs`, its `[[bench]]` entry, and the
   `papaya`, `dashmap` and `scc` dev-dependencies it added.

### Phase 4: object-cache

8. `rust/object-cache/src/metric_tags.rs`: delete `PrefixTags`, `EvictionTags` and their sets.
   Keep the label constants, `longest_prefix_match`, and a label table: `EvictionTagTable`
   becomes a `classify(key) -> &'static str` over the configured labels.
9. `rust/object-cache/src/range_cache/mod.rs`: replace `prefix_tags` / `other_tags` with the
   labels; delete `classify_tags` in favor of the existing `classify` (`:179`).
10. `rust/object-cache/src/range_cache/fetch.rs` and `foyer_backend.rs`: write
    `prefix = static label` (plus `class = static class` / `reason = static reason_str(reason)`)
    at each spread site, including `range_cache/mod.rs:241-298` (`prefix_tag` becomes the label
    from `classify`); `probe_blocks`, the function at `fetch.rs:267` and the helper at
    `fetch.rs:384` take labels instead of sets.
11. `CHANGELOG.md`: Unreleased entry with before/after numbers from step 6, plus a
    **Minor breaking change** clause for the removed `PrefixTags` / `EvictionTags`.

## Files to Modify

- `rust/Cargo.toml`, `rust/Cargo.lock`, `rust/datafusion-wasm/Cargo.lock`
- `rust/tracing/Cargo.toml`
- `rust/tracing/src/lib.rs`
- `rust/tracing/src/property_set.rs`
- `rust/tracing/src/static_set_cache.rs` (new)
- `rust/tracing/src/macros.rs`
- `rust/tracing/benches/properties.rs`
- `rust/tracing/benches/static_values.rs` (delete)
- `rust/tracing/tests/property_macro_tests.rs`
- `rust/tracing/tests/static_set_cache_tests.rs` (new)
- `rust/object-cache/src/metric_tags.rs`
- `rust/object-cache/src/range_cache/mod.rs`
- `rust/object-cache/src/range_cache/fetch.rs`
- `rust/object-cache/src/foyer_backend.rs`
- `rust/object-cache/tests/metric_tags_tests.rs` (delete `prefix_tags_carry_expected_properties`)
- `CHANGELOG.md`

## Benchmarks

Prototype (`rust/tracing/benches/static_values.rs`, on top of #1679). Each call emits
`imetric!` with a `prefix` picked per call from leaked `&'static str` labels, plus a literal
`class`. Single thread, ns per call:

| Approach | 4 labels | 64 labels | 2 runtime props, 12 combos |
|---|---|---|---|
| Precomputed `..set` | 25 | 25 | 25 |
| Dynamic path | 97–103 | 95–103 | 110–118 |
| `find_or_create` every call | 57 | 58 | 70 |
| Cache: `RwLock<FxHashMap>` | 36 | 35 | 35 |
| Cache: `scc::HashIndex` | 30–33 | 31 | — |
| Cache: `papaya` | 35 | 36 | — |
| Cache: `dashmap` | 38 | 36 | — |
| Cache: hand-rolled lock-free table | 25 | 25 | 25 |
| Thread-local map | 25 | 25 | — |

8 threads, ns per round of 8 calls (±100 ns run to run): precomputed 680–775, hand-rolled
690–810, thread-local 775–790, `scc` 960, `papaya` 1010, `RwLock` 1060, `dashmap` 1080, dynamic
1975–2315. The metric stream mutex dominates under contention.

## Trade-offs

- **Make runtime values static only in the merge (no cache)**: the dynamic path's cost is the
  merge `Vec`, sort and content hash, not string copies, so this saves block bytes but almost no
  CPU.
- **Intern on every call**: 57–70 ns and a global interner lock per call.
- **Hand-rolled lock-free table**: as fast as `..set`, but ours to keep correct under weak memory
  models on hardware we can't test (Android and others).
- **Thread-local map**: as fast, no atomics, but each thread warms up separately (short-lived
  threads approach intern-every-call) and logging during thread teardown needs a fallback.
- **`papaya`, `dashmap`, `RwLock<HashMap>`**: slower than `scc` and no simpler to use.
- **`scc::HashIndex`**: chosen. Maintained, loom-tested, one warm cache shared by all threads;
  ~5–8 ns over `..set` and two transitive crates.

## Decisions

- `scc::HashIndex` backs the cache (user call, over the hand-rolled table and thread-local map).
- With `..set` and a static value, literals are interned over the set rather than copied into the
  block. Without a static value, the `..set` expansion is unchanged.
- Speeding up the dynamic path itself (stack buffer instead of the merge `Vec`) is a separate
  follow-up (#1682).

## Documentation

- Rustdoc for `imetric!` and `log!` in `rust/tracing/src/macros.rs` (`fmetric!` and the level
  macros point to them).
- Module docs of `rust/tracing/src/property_set.rs`: extend the cardinality contract to `static`
  values.
- `rust/tracing/README.md:25`: mention `static` alongside `key = value`.
- No mkdocs page documents the property syntax.

## Testing Strategy

All no-DB unit tests and doctests.

- `rust/tracing/tests/static_set_cache_tests.rs` (cache directly):
  - same values return the same `&'static PropertySet` pointer;
  - different values return different sets with the expected properties;
  - equal content at two addresses returns two entries but one interned set;
  - the base set is part of the key, and pairs override its entries;
  - duplicate names among pairs resolve last-wins case-insensitively;
  - 8 threads racing on a cold key all get the same set.
- `rust/tracing/tests/property_macro_tests.rs`:
  - `static` on `imetric!`, `fmetric!` and `info!`, alone and mixed with literals, dynamic
    values and `..set`, checking recorded properties;
  - extend `duplicate_keys` with static/literal in both orders, static/dynamic, and static over
    `..set`, plus `info!(k = x, k = "b", ..set, …)` with and without a `p = static s` pair
    (`k = "b"` without, `k = x` with);
  - a static-only call produces an interned set id and no dynamic dependency records (it takes the
    `..set` path, not the dynamic one);
  - a disabled log doesn't evaluate its `static` expression (side-effect counter), like the
    existing gating test for dynamic values (`guard.disable_target`).
- `compile_fail` doctests: a borrowed `&str` (`k = static s.as_str()` with `s: String`), a
  reserved key (`unit = static x`), and `static` with `%` / `?`.
- `cargo bench -p micromegas-tracing --bench properties` before (save baseline) and after;
  numbers go in the CHANGELOG entry.
- object-cache's existing tests cover the migrated call sites.

## Manual Verification

1. Temporarily add `info!(k = static "v", "x")` to `rust/datafusion-wasm/src/lib.rs`, then from
   `rust/datafusion-wasm` run `cargo clippy --target wasm32-unknown-unknown -- -D warnings`.
   Expected: clean. Revert the line. CI's WASM clippy step builds the stub module but no
   wasm-compiled code uses `static` yet, so the macro expansion against the stub is only checked
   here.

## Open Questions

None.
