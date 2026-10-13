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
  outright. The `properties: set` log arm and the 4-argument metric arm have about 78 non-test
  callsites (77 four-argument `imetric!`/`fmetric!` plus one `properties:` log) across 14
  files, mostly `redis-exporter/src/sampler.rs` (31), `public/src/servers/pg_stats.rs` (13),
  `object-cache/src/foyer_backend.rs` (7), `object-cache/src/range_cache/fetch.rs` (6),
  `object-cache-srv/src/handlers.rs` (5), `redis-exporter/src/main.rs` (4) and
  `object-cache/src/range_cache/mod.rs` (4). `object-cache/src/metric_tags.rs` only builds
  sets. Removing the arms is a Rust API break (see Recommendation), which the project
  accepts.
- **Existing callers by migration target:**
  - Callers that rebuild an all-literal set on every call, which become kv literals: the
    4-argument callsites whose set is built from literals inline. Each call pays a `Vec`
    allocation, a sort and the global intern lock; this drops to one atomic load.
  - `ingestion/src/web_ingestion_service.rs:423` moves to `imetric!(target: ..)` and carries no
    properties, so the `target:` arm removes its per-call `find_or_create` entirely.
  - `class_tags` (`object-cache/src/metric_tags.rs:77`) takes a runtime `class_label`, so a
    literal arm can't match it. Its callers (`range_cache/fetch.rs:326`, `:340`) already use
    the precomputed `run_demand_tag` / `run_prefetch_tag` sets (`fetch.rs:318-322`) and move to
    `..set`.
  - Callers that `intern_string` bounded runtime values per call become dynamic kv pairs, so the
    interning goes away: `pg_stats.rs:46-59`, `view_registry.rs:43-48`,
    `query_deny_list.rs:355-360`, `maintenance.rs:103-106`. Where a set is already precomputed
    once (not rebuilt per call) it becomes `..set` instead. `maintenance.rs:105` interns
    `view_instance_id`, which looks unbounded; as a dynamic kv value it is borrowed and the
    leak disappears.
  - `object-cache-srv/src/handlers.rs:200/255/400/463/609` do a per-call `find_or_create`
    with runtime values; they become dynamic kv pairs.
  - `redis-exporter/src/sampler.rs:22-33` interns user-supplied property names and values.
    The names are runtime, so it stays on the function API (`DynProperty` with interned
    names) or on interned sets passed as `..set`.
  - `public/src/utils/log_json_rows.rs` builds runtime property *names*, so it stays on the
    function API (`log_dyn_tagged` + `DynProperty`) with interned names and borrowed values.
    This also removes the per-row value-interning leak. It relies on the log `target`
    override (`Property::new("target", target)`), which survives through `DynProperty`.

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
- An integer literal value put in a `&'static str` slot fails to compile. Building the static
  set in the `OnceLock` initializer with `intern_string(&$v.to_string())` avoids this, so
  `n = 3` and `ok = true` work too.
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
  - `%expr` (Display) and `?expr` (Debug) cost a `String` allocation per call, and are only
    evaluated when the level is enabled.

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
  muncher handles this with an extra `{set}` accumulator (prototyped).
- **Routing:** calls with no properties keep today's untagged path (`log!` to
  `dispatch::log`, the 3-argument metric arm to `int_metric` / `float_metric`). Only
  property-bearing calls go through the `*_dyn` entry point with one static set (the spread
  set, the callsite literal set, or `PropertySet::empty()`) and the dynamic slice. Dispatch
  takes the arena-free path when the slice is empty. The cases that are optimal today keep
  their cost:
  - `..set` only: today's `properties: set` (pointer pass-through, no merge, no arena);
  - literals only: the same, after one atomic load of the per-callsite `OnceLock`. This beats
    today's per-call `find_or_create` callers that build all-literal sets inline;
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
  - **Macros reject them.** Arms ahead of the generic kv arms match `level`, `target`, `name`
    and `unit` as ident keys or as string-literal keys, and expand to a `compile_error!` that
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
  needs wasm-compatible expansion (it only changes the static metadata).

## Implementation Steps

1. Per-callsite literal-set helper (`OnceLock<&'static PropertySet>` filled by `find_or_create`)
   in `rust/tracing`, plus the matching wasm32 stub next to the `property_set` stub in
   `rust/tracing/src/lib.rs` (~157-177).
2. Short-circuit an empty dynamic slice before `merge_properties` in
   `rust/tracing/src/dispatch.rs` (or in the free functions).
3. Rewrite `rust/tracing/src/macros.rs`: `log!`, `imetric!`, `fmetric!` and the level macros
   (kv muncher, `..set`, reserved-key `compile_error!` arms, metric `target:` arm).
4. Migrate the ~78 callsites (see Constraints) to kv pairs or `..set`; keep `log_json_rows.rs`
   and the `redis-exporter` sampler on the function API.
5. `analytics/src/log_entry.rs`: keep the descriptor level and warn on an unparseable `level`
   override.
6. Tests (below).
7. Docs and CHANGELOG (below).

### Tests
- Update `rust/tracing/tests/test_macros.rs` (`properties:` at 29-45, 4-argument metrics at
  86-103) to the new syntax.
- Each kv form: literal, expr, `%`, `?`, string-literal key, `..set`, `..set` mixed with kv.
- Pointer equality of the per-callsite literal set across calls.
- Named format args after the format string.
- Metric `target:` arm.
- No-property calls still emit untagged events.
- `compile_fail` doctests in `macros.rs` (none exist today) for reserved keys and a double
  `..set`.
- No-DB unit test in `analytics/tests/log_tests.rs` for the invalid `level` override.

### Docs
- `rust/tracing/src/lib.rs:51-76` (Dynamic properties section; lines 73-75 say an invalid level
  fails the block, which is wrong after the analytics fix).
- `rust/tracing/src/macros.rs` doc comments (~163-180, ~239, ~308-322).
- `rust/tracing/README.md:25`.
- `CHANGELOG.md:7-9` (Unreleased entry): rewrite for kv syntax, `..set`, the metric `target:`
  arm and reserved-name rejection, with the **Minor breaking change** clause.
