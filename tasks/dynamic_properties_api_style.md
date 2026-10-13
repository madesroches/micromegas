# Dynamic Properties: More Idiomatic Call Syntax

Follow-up to `tasks/completed/1623_dynamic_properties_plan.md`.

## Problem
Attaching a dynamic property to a metric today looks like this:

```rust
imetric!(
    "rows_fetched",
    "count",
    PropertySet::empty(),
    &[DynProperty { name: "correlation_id", value: &correlation_id }],
    3
);
```

and to a log entry: `info!(properties: set, dyn_properties: &[DynProperty { .. }], "...")`.

Issues:
- The caller supplies two property arguments: an interned `PropertySet` and a `DynProperty`
  slice. They have to pass `PropertySet::empty()` when they only have dynamic values.
- Struct-literal slices are verbose compared with the key-value syntax of other libraries.
- Choosing the cheap path is left to the caller. If every value is a `'static` string, the set
  should be interned automatically instead of being copied into the block's arena.

## Survey

| Library | Per-call syntax | Keys | Values | Static/dynamic handling |
|---|---|---|---|---|
| `tracing` 0.1 | `info!(cid = %cid, n = 3, "msg {}", x)`: fields first, comma-separated, then the format string | ident, dotted ident (`http.method`) or string literal; stored `'static` in the callsite `FieldSet` | borrowed `&dyn Value` per call; `%` = Display, `?` = Debug, shorthand `info!(cid, ..)` | names are static by construction, values are always borrowed. No special case for literal values. The subscriber decides what to copy. |
| `log` 0.4 (`kv`) | `info!(cid = cid, n:? = n; "msg")`: `;` ends the key-values | `$key:tt`: ident/literal → `Key::from_str_static`, otherwise `Key::from_str` | captured by reference (`capture_to_value(&&v)`); `:?`, `:%`, `:serde`, `:sval`, `:err` modifiers | none. Everything is borrowed for the duration of the call. |
| `slog` 2 | `info!(log, "msg"; "k" => v, "k2" => %v)`; context via `o!(...)` on a `Logger` | `&'static str` (unless `dynamic-keys`) | `Value` trait, `%`/`?`/`#?` sigils, `FnValue` for lazy values | per-logger `OwnedKV` (built once, inherited by child loggers) vs per-record borrowed kvs |
| OpenTelemetry 0.32 | `counter.add(3, &[KeyValue::new("k", v)])` | `Key(OtelString)`: `Static(&'static str)` / `Owned(Box<str>)` / `RefCounted(Arc<str>)` | same enum for strings, plus i64/f64/bool/arrays | **runtime, by type**: `From<&'static str>` stays static, `From<String>` is owned. A borrowed non-`'static` `&str` is rejected, so the caller has to allocate. The SDK still hashes the attributes per call. |
| `metrics` | `counter!("name", "k" => v).increment(3)` | `SharedString` (Cow-like) | same | **compile time, in the macro**: `key_var!` has a `$k:literal => $v:literal` arm listed before the `$k:expr => $v:expr` arm. All-literal labels expand to `static LABELS: [Label; N] = [Label::from_static_parts(..)]` + `static METRIC_KEY: Key = Key::from_static_parts(..)`. Any expression falls back to `Label::new` in a `Vec` per call. A prebuilt-labels arm `(name, labels_expr)` exists too. |

Takeaways:
- Everyone uses inline `key = value` / `"key" => value` syntax. Struct literals only appear in
  function APIs (OTel's `&[KeyValue]`).
- Keys are static everywhere that cares about cost (tracing, slog, our `DynProperty`).
- Only `metrics` does the "all values static" optimization, and it does it syntactically in
  `macro_rules`. OTel decides by type, but that only saves a copy and forces `String`s for
  borrowed dynamic values.

## Constraints in this codebase

- **A runtime static check isn't possible.** Lifetimes are erased and stable Rust has no
  specialization, so `&'static str` and `&'a str` can't take different paths. The OTel
  workaround (accept only `'static` or owned values) would force a `String` allocation for the
  main use case: a borrowed correlation id.
- **A fully `const` set isn't possible either.** `StaticStringRef.ptr` is a `u64` address
  (`static_string_ref.rs`), and pointer-to-int casts aren't allowed in `const`. `PropertySet`
  also holds a `Vec` and is interned through `internment`. A literal set can therefore only be
  built lazily, per callsite: a `static OnceLock<&'static PropertySet>` filled by
  `find_or_create` on the first call. After that each call costs one atomic load. Interning
  keeps identical sets at one address across callsites.
- **The dispatch layer needs no change.** `log_dyn_tagged` / `tagged_*_metric_dyn` already take
  `(&'static PropertySet, &[DynProperty])`, and `merge_properties` lets a dynamic value override
  a static one. The one useful addition is a short-circuit to the `log_tagged` /
  `tagged_*_metric` path when the slice is empty. The branch is predictable and keeps the
  macro to a single expansion.
- **Nothing has shipped.** `dyn_properties:` and the 5-argument metric arms exist only on this
  branch (CHANGELOG `Unreleased`, no callers outside tests and docs). They can be replaced
  outright. The `properties: set` log arm and the 4-argument metric arm have about 77 non-test
  callsites (76 four-argument `imetric!`/`fmetric!` plus one `properties:` log) across 14
  files, mostly `redis-exporter/src/sampler.rs` (31), `public/src/servers/pg_stats.rs` (13),
  `object-cache/src/foyer_backend.rs` (7), `object-cache/src/range_cache/fetch.rs` (6),
  `object-cache-srv/src/handlers.rs` (5), `redis-exporter/src/main.rs` (4) and
  `object-cache/src/range_cache/mod.rs` (4). `object-cache/src/metric_tags.rs` only builds
  sets. Removing the arms is a Rust API break (see Recommendation), which the project
  accepts.
- **Existing callers by migration target:**
  - `ingestion/src/web_ingestion_service.rs:423` moves to `imetric!(target: ..)` and carries no
    properties, so the `target:` arm removes its per-call `find_or_create` entirely.
  - `class_tags` (`object-cache/src/metric_tags.rs:77`) is deleted. Its callers
    (`range_cache/fetch.rs:326`, `:340`) become a dynamic kv pair `class = class` (a runtime
    `&'static str`). They must not use `..run_class_tag`: that set carries a prefix as well, and
    these latency metrics are deliberately not split by prefix (`metric_tags.rs:72-76`).
  - Callers that already pass a precomputed set (about 50 calls: `foyer_backend.rs` 7,
    `range_cache/mod.rs` 4, `fetch.rs:164/197/416/422`, `redis-exporter/src/main.rs` 4, most of
    `sampler.rs`) become `..set` with no other change.
  - `rule_tags` callers (including `public/src/servers/flight_sql_service_impl.rs:828`) become
    `..set` or dynamic kv pairs, depending on whether the set is precomputed.
  - `auth/src/db_api_key.rs:390` (`table_tags`, a per-call `find_or_create` defined at `:215`)
    becomes a dynamic `table = ..` pair.
  - Callers that `intern_string` bounded runtime values per call become dynamic kv pairs, so the
    interning goes away: `pg_stats.rs:46-59`, `view_registry.rs:43-48`,
    `query_deny_list.rs:355-360`, `maintenance.rs:103-106`. Where a set is already precomputed
    once (not rebuilt per call) it becomes `..set` instead. `maintenance.rs:105` interns
    `view_instance_id`, which looks unbounded; as a dynamic kv value it is borrowed and the
    leak disappears.
  - `object-cache-srv/src/handlers.rs:200/255/400/463/609` do a per-call `find_or_create`
    with runtime values; they become dynamic kv pairs.
  - `redis-exporter/src/sampler.rs:22-33` interns user-supplied property names and values. The
    realistic path is `..props` plus kv pairs (e.g. `..props, event = event` for `with_extra`)
    rather than hand-written `StaticMetricMetadata` at its 31 callsites.
  - `public/src/utils/log_json_rows.rs` builds runtime property *names*, and today uses
    `info!(properties: pset, ..)` (interning both name and value per row). It migrates to
    `dispatch::log_dyn_tagged`, which takes a `&'static LogMetadata`: it needs a hand-written
    `static LogMetadata` and the `STATIC_MAX_LEVEL` / `max_level()` gate the macro provided.
    Column names stay `intern_string`'d (bounded); values are borrowed, which removes the
    per-row value-interning leak. It relies on the log `target` override
    (`Property::new("target", target)`), which survives through `DynProperty`.

## Prototype

I checked the macro mechanics in a throwaway crate (stable Rust, edition 2024) with a
`macro_rules` tt-muncher that sorts each pair into a static bucket or a dynamic bucket:
- `$k:ident = $v:literal` and `$k:literal = $v:literal` (listed before the `expr` arm) go to
  the static bucket. `$k = %$v`, `$k = ?$v` and `$k = $v:expr` go to the dynamic bucket.
  Unlike `expr`, a `literal` fragment that doesn't match falls through to the next arm cleanly.
- Temporaries such as `&*$v.to_string()` inside the inline `&[DynProperty { .. }]` array live
  until the end of the dispatch call, so no `let` bindings or generated identifiers are needed.
- Both shapes parse: tracing-style `k = v, .., "fmt", args` (a string-literal key is told apart
  from the format string by the following `=`) and log-style `k = v; "fmt", args`.
  Named format args (`x = 1` after the format string) still work.
- An integer literal value put in a `&'static str` slot fails to compile, but `concat!($v)`
  through a `$v:literal` fragment is a compile-time `&'static str` for string, integer
  (including negative, suffixed and hex), float, bool and char literals (only byte strings
  fail). So `n = 3` and `ok = true` work with no allocation. `concat!` keeps a float's
  spelling (`1e3` stays `1e3`), whereas `to_string()` would normalize it.
- A `.. $set:expr` element parses next to the kv arms (including `..t.prefix` field access),
  and an accumulator records whether a set was spread.
- Each pair adds one level of recursion (the default limit is 128). A final
  `compile_error!("...")` arm is needed for readable errors.

## Recommendation

**One key-value list, in the `tracing` style, with the literal/dynamic split done at compile
time in the macro (the same approach as `metrics`).**

```rust
info!(correlation_id = cid, region = "eu", rows = %n, "fetched {n} rows");
imetric!("rows_fetched", "count", correlation_id = cid, region = "eu", n);
fmetric!("fetch_ms", "ms", "http.method" = "GET", elapsed_ms);
```

- **Syntax:** `tracing`-style leading fields for logs, because the `tracing` interop already
  maps `tracing::info!(correlation_id = %cid, "...")` fields to the same properties. The two
  macros then read the same and produce the same rows. For metrics, the fields go between the
  unit and the value, so the value stays last as in the 3-argument arm. The kv arms must come
  before `($name, $unit, $value)`, because `$value:expr` fails hard on `k = %v`.
- **`target:`** (logs and metrics) is a separate arm that takes no properties, as today.
- **Keys:** an ident, which is `stringify!`-ed, or a string literal for dotted or semconv names.
  Keys are always `'static`.
- **Values:**
  - a literal goes into the per-callsite interned set, built once and never copied into the
    arena;
  - `expr` (`AsRef<str>`: `&str`, `String`) is borrowed with no copy, and the arena copies it
    once per block;
  - `%expr` (Display) and `?expr` (Debug) cost a `String` allocation per call. Log macros
    evaluate them only when the level is enabled; metrics always do.

  `tracing`'s bare-ident shorthand (`info!(cid, "...")`) is left out for now and can be added
  later.
- **Precomputed sets** go in the same list as a spread element, `..set`, where `set` is a
  `&'static PropertySet`. This replaces `properties: set` and the 4-argument metric arm, so
  each macro takes exactly one property argument:

  ```rust
  imetric!("object_cache_disk_tier_hit", "count", ..t.prefix, 1_u64);
  imetric!("range_cache_block_request", "count", ..block_tag, correlation_id = cid, 1_u64);
  imetric!("object_cache_disk_tier_miss", "count", ..t.prefix, 1_u64);
  ```

  A list can hold at most one `..set`. With a spread present, literal pairs go to the dynamic
  bucket, because merging two static sets would cost an intern lookup on every call. The
  muncher handles this with an extra `{set}` accumulator (prototyped). In that dynamic bucket,
  literal values use `concat!($v)` as well, so there is no per-call allocation.
- **Routing:** calls with no properties keep today's untagged path (`log!` to
  `dispatch::log`, the 3-argument metric arm to `int_metric` / `float_metric`). Only
  property-bearing calls go through the `*_dyn` entry point with one static set (the spread
  set, the callsite literal set, or `PropertySet::empty()`) and the dynamic slice. Dispatch
  takes the arena-free path when the slice is empty. The cases that are optimal today keep
  their cost:
  - `..set` only: today's `properties: set` (pointer pass-through, no merge, no arena);
  - literals only: the same, after one atomic load of the per-callsite `OnceLock`;
  - dynamic values: today's `dyn_properties:` cost (one merge and content hash, plus an arena
    copy once per block).
- **Removed (breaking):** `properties:` (logs), the 4-argument `imetric!`/`fmetric!` arm,
  `dyn_properties:` and the 5-argument arms. All in-repo callers move to `..set` or kv pairs.
  The CHANGELOG entry gets a **Minor breaking change** clause for the `properties:` / 4-argument
  forms. The `dyn_properties:` forms were never released and just disappear from the entry.
- **Kept:** the dispatch functions (`log_tagged`, `log_dyn_tagged`, `tagged_*_metric[_dyn]`)
  and `DynProperty`, for lists built at runtime (`tracing` interop, `log_json_rows`).
- **Unchanged:** the SQL surface, the wire format, and the arena.
- **Reserved names.** Analytics treats some property names as column overrides:
  - logs (`analytics/src/log_entry.rs`, `TaggedLogString` arm): `target` and `level`;
  - metrics (`analytics/src/measure.rs`, tagged arms): `target`, `name` and `unit`.

  The property also stays in `properties`. `TaggedLogInteropEvent` doesn't apply overrides.
  Analytics matches these names case-sensitively, whereas `merge_properties` and
  `property_get` compare case-insensitively. Changes:
  - **Macros reject them.** Arms ahead of the generic kv arms match, for the log macros,
    `level` and `target`, and for `imetric!`/`fmetric!`, `target`, `name` and `unit`, as ident keys or as string-literal keys, and expand to a `compile_error!` that
    names the real argument. This keeps `info!(target = x, ..)` from being confused with
    `info!(target: x, ..)`. The macros can't see inside a `..set` spread or a runtime
    `DynProperty` slice, so overrides through those still work; this is the documented escape
    hatch.
  - **Metric macros gain `target:`** (`imetric!(target: "micromegas::ingestion", "name",
    "unit", v)`), matching the log macros. It sets `StaticMetricMetadata::target` instead of
    `module_path!()`. Like the log `target:` arm today (`macros.rs:325` vs the property arms),
    it takes no properties and does not combine with kv pairs or `..set`.
    `web_ingestion_service.rs:423`, the only in-repo *metric* caller of the `target` override,
    moves to it and needs no properties. `log_json_rows.rs` also relies on the log `target`
    override, which it keeps only through the function / `DynProperty` path or a spread.
  - **Analytics keeps the override** (SQL and wire format are stable, and blocks already
    ingested rely on it). One fix: an unparseable `level` override no longer fails the whole
    block. `log_entry_from_value` keeps the descriptor's level and logs a warning instead of
    `Level::parse(..)?`. Before, one bad dynamic value dropped every event in its block.
    This needs a no-DB unit test in `analytics/tests/log_tests.rs`: a block with an invalid
    `level` property parses, and the row keeps the descriptor level.
- **Other notes:** the wasm stubs need the callsite-set helper. The `target:` metric arm also
  needs wasm-compatible expansion (it only changes the static metadata). On wasm32 the
  `property_set` stub has no `Property::new` / `find_or_create`, and `intern_string` /
  `static_string_ref` are native-only, and a closure passed to the helper is still type-checked
  there. The per-callsite `OnceLock<&'static PropertySet>` must be a `static` emitted by the
  macro expansion (a `static` inside a plain helper fn would be shared by all callsites); it is
  passed to the helper with the `&'static [(&'static str, &'static str)]` literal pairs (built
  with `concat!`), or both are held in a const-constructible helper struct. The expansion may
  name `PropertySet`, `OnceLock`, the helper, `DynProperty` and the dispatch functions (the
  wasm32 stub defines `PropertySet` and `OnceLock::new()` is const), and must avoid only
  `Property::new` / `find_or_create` / `intern_string` / `static_string_ref`, so the wasm32
  stub can ignore its input.

## Implementation Steps

1. Per-callsite literal-set helper in `rust/tracing` (fills the macro-emitted per-callsite
   `static OnceLock<&'static PropertySet>` via `find_or_create`), plus the matching wasm32 stub
   next to the `property_set` stub in `rust/tracing/src/lib.rs` (~157-177). The helper takes the
   `OnceLock` static and a `&'static [(&'static str, &'static str)]` (values via `concat!`), or a
   const-constructible struct holding both, so the macro never names `Property::new`,
   `find_or_create`, `intern_string` or `static_string_ref`.
2. Short-circuit an empty dynamic slice before `merge_properties` in
   `rust/tracing/src/dispatch.rs` (or in the free functions).
3. Rewrite `rust/tracing/src/macros.rs`: `log!`, `imetric!`, `fmetric!` and the level macros
   (kv muncher, `..set`, reserved-key `compile_error!` arms, metric `target:` arm).
4. Migrate the ~77 callsites (see Constraints) to kv pairs or `..set`; move `log_json_rows.rs` to
   `dispatch::log_dyn_tagged` with a hand-written `static LogMetadata` and the level gate, and
   the `redis-exporter` sampler to `..props` plus kv pairs.
5. `analytics/src/log_entry.rs`: keep the descriptor level and warn on an unparseable `level`
   override.
6. Tests (below).
7. Docs and CHANGELOG (below).

### Tests
- Update `rust/tracing/tests/test_macros.rs` (`properties:` at 29-45, 4-argument metrics at
  86-103) to the new syntax. The calls at `:90` and `:99` pass `Property::new("name", ..)`;
  they keep `name` through a `..set` spread, as a deliberate test of the escape hatch
  (`name = ..` would hit the reserved-key `compile_error!`).
- Each kv form: literal, expr, `%`, `?`, string-literal key, `..set`, `..set` mixed with kv.
- Pointer equality of the per-callsite literal set across calls.
- Named format args after the format string.
- Metric `target:` arm.
- A cfg-gated test in `rust/datafusion-wasm` (covered by `run_wasm()` in `build/rust_ci.py`,
  which only runs fmt/clippy/tests there) that expands the kv, `..PropertySet::empty()` (the
  only `&'static PropertySet` on wasm32) and metric `target:` arms under
  `wasm32-unknown-unknown`.
- No-property calls still emit untagged events.
- `compile_fail` doctests in `macros.rs` (none exist today) for reserved keys and a double
  `..set`.
- No-DB unit test in `analytics/tests/log_tests.rs` for the invalid `level` override.

### Docs
- `rust/tracing/src/lib.rs:51-76` (Dynamic properties section; lines 74-75 say an invalid level
  fails the block, which is wrong after the analytics fix).
- `rust/tracing/src/property_set.rs:30-33` (`DynProperty` doc says an invalid dynamic level
  fails the block; same fix).
- `rust/auth/src/db_api_key.rs:248` (doc comment showing the 4-argument `imetric!` form).
- `rust/tracing/src/macros.rs` doc comments (~163-180, ~239, ~308-322).
- `rust/tracing/README.md:25`.
- `CHANGELOG.md:7-20` (Unreleased entry): rewrite for kv syntax, `..set`, the metric `target:`
  arm and reserved-name rejection, appending to the existing **Minor breaking change** clause
  at 14-20.

## Decisions
- `class_tags` is deleted; its callers use a dynamic `class = class` pair.
