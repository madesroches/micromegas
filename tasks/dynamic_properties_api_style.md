# Dynamic Properties: More Idiomatic Call Syntax

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1623

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
  `macro_rules`. OTel decides by type.

## Constraints in this codebase

- **A runtime static check isn't possible.** Lifetimes are erased and stable Rust has no
  specialization, so `&'static str` and `&'a str` can't take different paths. The OTel
  workaround (accept only `'static` or owned values) would force a `String` allocation for the
  main use case: a borrowed correlation id.
- **A fully `const` set isn't possible either.** `StaticStringRef.ptr` is a `u64` address
  (`static_string_ref.rs`), and pointer-to-int casts aren't allowed in `const`. `PropertySet`
  also holds a `Vec` and is interned through `internment`. A literal set can therefore only be
  built lazily, per callsite: a `static OnceLock<&'static PropertySet>` filled by
  `find_or_create` on the first call. After that each call costs one atomic load.
- **The dispatch layer needs no change.** `log_dyn_tagged` / `tagged_*_metric_dyn` already take
  `(&'static PropertySet, &[DynProperty])`, and `merge_properties` lets a dynamic value override
  a static one. The one useful addition is a short-circuit to the `log_tagged` /
  `tagged_*_metric` path when the slice is empty. The branch is predictable and keeps the
  macro to a single expansion.
- **Nothing has shipped.** `dyn_properties:` and the 5-argument metric arms exist only on this
  branch (CHANGELOG `Unreleased`, no callers outside tests and docs). They can be replaced
  outright. The `properties: set` log arm and the 4-argument metric arm have about 76 non-test
  callsites (76 four-argument `imetric!`/`fmetric!`; the one `properties:` log, in
  `log_json_rows.rs`, is deleted with that file) across 13 files. Removing the arms is a Rust API break (see Recommendation), which the project
  accepts.
- **Existing callers by migration target:**
  - `ingestion/src/web_ingestion_service.rs:420` moves to `imetric!(target: "micromegas::ingestion", ..)`
    with no properties, so the `target:` arm removes its per-call `find_or_create` entirely. This
    drops the overridden `target` property: `property_get(properties,'target')` returns NULL for
    `payload_size_inserted` afterwards (the `target` column is unchanged). The two-line comment
    at 414-415 ("this measure does not benefit from a dynamic property...") goes stale and is
    deleted with the migration; the comment at 416-418 (explaining the `PutIfAbsent::Created`
    guard) stays. The `use micromegas_tracing::property_set;` import (:15) is deleted.
  - `class_tags` (`object-cache/src/metric_tags.rs:77`) is deleted. Its callers
    (`range_cache/fetch.rs:326`, `:340`) become a dynamic kv pair `class = class` (a runtime
    `&'static str`). They must not use `..run_class_tag`: that set carries a prefix as well, and
    these latency metrics are deliberately not split by prefix (`metric_tags.rs:72-76`).
  - Callers that already pass a precomputed set (43 calls: `foyer_backend.rs` 7,
    `range_cache/mod.rs` 4, `fetch.rs` 4, `redis-exporter/src/main.rs` 4, and 24 `sampler.rs`
    calls passing `props`) become `..set` with no other change. The 7 `sampler.rs` calls through
    `with_extra` belong to the sampler bullet below.
  - `rule_tags` (analytics, `query_deny_list.rs` ~616-619 and `flight_sql_service_impl.rs`
    ~828) builds the set per call at both callers: the `query_deny_list.rs` caller becomes
    `rule_id = %rule_id` and the `flight_sql_service_impl.rs` caller `rule_id = %rule.row.rule_id`;
    `rule_tags`, its doc comment (`query_deny_list.rs:352-355`), `rule_tags` in the
    grouped `use` list at `flight_sql_service_impl.rs:43`, and the `intern_string` (:21) and `Property, PropertySet`
    (:23) imports in `query_deny_list.rs` are deleted.
  - `auth/src/db_api_key.rs:390` (`table_tags`, a per-call `find_or_create` defined at `:215`)
    becomes a dynamic `table = ..` pair, and `table_tags` is deleted. The values are `&'static str` from a fixed table set,
    and as dynamic kv they pay a merge plus an arena copy; that cost is accepted.
  - Callers that `intern_string` bounded runtime values per call become dynamic kv pairs, so the
    interning goes away: the `pg_stats.rs` callers (:84, :137, :267; at :84 and :137 the pairs
    repeat at each of the 12 `imetric!` calls sharing the `tags` local), the `view_registry.rs`
    caller (~:105),
    `maintenance.rs:103-106` (its `Arc<String>` value is a bare value). The helpers and imports
    left dead are deleted with their doc comments: in `pg_stats.rs` `table_tags` (:45),
    `index_tags` (:50), `state_tags` (:58) and the `intern_string` (:22) and
    `Property, PropertySet` (:24) imports; in `view_registry.rs` `failure_tags` (:43) and its
    imports (:17, :19); in `public/src/servers/maintenance.rs` the `intern_string` (:16) and
    `Property, PropertySet` (:18) imports.
  - `object-cache-srv/src/handlers.rs:200/255/400/463/609` do a per-call `find_or_create`
    with runtime values; they become dynamic kv pairs, and the `Property, PropertySet` import
    (:18) is deleted.
  - `redis-exporter/src/sampler.rs:22-33` interns user-supplied property names and values. The
    realistic path is kv pairs plus a trailing `..props` (`with_extra`'s three callsites become
    `event = event, ..props`, `db = %entry.db, ..props` (`db` is a `u32`, which has no bare
    `PropertyValue` impl) and `command = stat.name, ..props`)
    rather than hand-written `StaticMetricMetadata` at its 31 callsites. `with_extra` (private)
    has no callers left and is deleted.
  - `public/src/utils/log_json_rows.rs` is deleted, along with its `pub mod log_json_rows;` line
    in `public/src/utils/mod.rs`. That leaves `utils/mod.rs` empty and the `utils` module unused,
    so `utils/mod.rs` and the `pub mod utils;` line in `public/src/lib.rs` are removed too (lines 189-190, together with its
    `#[cfg(feature = "server")]` attribute).

## Prototype

I checked the macro mechanics in a throwaway crate (stable Rust, edition 2024) with a
`macro_rules` tt-muncher that sorts each pair into a static bucket or a dynamic bucket:
- Keys are matched as `$k:tt =`, and values `$v:literal` go to the static bucket. A value
  starting with `-` that is not a literal (`k = -x`) hard-errors in a `$v:literal` arm, so
  `$k = - $v:literal` (concat with a leading `-`) and a fallback come ahead of the plain
  `$v:literal` arm. The fallback forwards `$k = - $($rest)*` unchanged to an expr-only arm, so
  the whole `-...` expression is parsed as one `$v:expr` and `-$v` is never emitted (`-$v` would
  turn `-a + b` into `-(a + b)`). `$k = %$v`,
  `$k = ?$v` and `$k = $v:expr` go to the dynamic bucket. A `tt` never fails fatally, whereas a
  `$k:literal` key arm turns a metric value starting with `-` (`fmetric!("n", "u", -x)`) into a
  hard parse error, since `-` can begin a literal. A helper classifies the key: an ident is
  `stringify!`-ed, a literal passes through, anything else is a `compile_error!`.
- Temporaries such as `&*$v.to_string()` inside the inline `&[DynProperty { .. }]` array live
  until the end of the dispatch call, so no `let` bindings or generated identifiers are needed.
- The tracing-style `k = v, .., "fmt", args` shape parses (a string-literal key is told apart
  from the format string by the following `=`). Named format args (`x = 1` after the format
  string) still work.
- An integer literal value put in a `&'static str` slot fails to compile, but `concat!($v)`
  through a `$v:literal` fragment is a compile-time `&'static str` for string, integer
  (including negative, suffixed and hex), float, bool and char literals (only byte strings
  fail). So `n = 3` and `ok = true` work with no allocation. `concat!` normalizes integer literals
  (`1_000` becomes "1000", `0x10` becomes "16", the suffix is dropped) but keeps a float's
  spelling (`0.50` stays "0.50"), whereas `%x` uses `Display`. This is a known inconsistency
  to document in the macro docs.
- A trailing `.. $set:expr` element parses after the kv arms (including `..t.prefix` field
  access), and an accumulator records whether a set was spread.
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
  macros then read the same and attach the same property names and values. Only the
  tracing-style comma form is accepted (no log-style `;`). Log and metric lists accept an
  optional trailing comma, and log macros still require a format string (no message-less
  events). For metrics, the fields go between the
  unit and the value, so the value stays last as in the 3-argument arm. The kv arms must come
  before `($name, $unit, $value)`, because `$value:expr` fails hard on `k = %v`.
- **`target:`** (logs and metrics) enters the same kv muncher, with the target as an
  accumulator that defaults to `module_path!()`: `info!(target: "x", k = v, "msg")` and
  `imetric!(target: "x", "n", "u", k = v, value)` work. The metric `target:` sets
  `StaticMetricMetadata::target` instead of `module_path!()`.
- **Keys:** an ident, which is `stringify!`-ed, or a string literal for dotted or semconv names.
  Keys are always `'static`.
- **Values:**
  - a literal goes into the per-callsite interned set, built once and never copied into the
    arena;
  - a bare `expr` implements `PropertyValue`, a project-owned trait in `micromegas-tracing`
    with a method returning `Cow<'_, str>` (strings borrow, future numeric impls can format).
    Explicit impls: `str`, `String`, `Cow<'_, str>`, and forwarding impls for `&T`, `Box<T>`,
    `Arc<T>`, `Rc<T>` where `T: PropertyValue + ?Sized` (covering `Arc<String>`, `Arc<str>`,
    `&String`). The expansion binds the `Cow` in a temporary that outlives the dispatch call and
    borrows `&*cow` into the `DynProperty` slice, so strings are borrowed with no copy and the
    arena copies them once per block. Non-string values (`Uuid`, `u32`) use `%` for now; adding
    std/integer/bool impls later is non-breaking because downstream crates can't implement the
    trait for foreign types (orphan rules);
  - `%expr` (Display) and `?expr` (Debug) cost a `String` allocation per call. When a log list has `%`/`?`
    or bare-value dynamic entries, the expansion also gates on
    `$crate::dispatch::log_enabled(&LOG_DESC)` after `STATIC_MAX_LEVEL` and `max_level()` and
    before building the slice (the pattern of
    `log_enabled!`, `macros.rs:545-565`; `dispatch_wasm.rs` has a stub), so they are
    evaluated only when the level and target are enabled; metrics always evaluate them.

  `tracing`'s bare-ident shorthand (`info!(cid, "...")`) is left out for now and can be added
  later.
- **Precomputed sets** go in the same list as a spread element, `..set`, where `set` is a
  `&'static PropertySet`. `..set` is allowed only last in the list (struct-update style) and at
  most once; any other position is a `compile_error!`. kv pairs override the set's entries. For
  duplicate keys, a dynamic value overrides a literal one with the same key regardless of
  position (`merge_properties` lets dynamic entries replace static ones); within each bucket the
  last pair wins, and the per-callsite literal-set helper dedups case-insensitively like
  `merge_properties` (`PropertySet::find_or_create` only sorts). With a trailing `..set` all
  pairs are dynamic, so the last wins. This
  replaces `properties: set` and the 4-argument metric arm, so
  each macro takes exactly one property argument:

  ```rust
  imetric!("object_cache_disk_tier_hit", "count", ..t.prefix, 1_u64);
  imetric!("range_cache_block_request", "count", correlation_id = cid, ..block_tag, 1_u64);
  imetric!("object_cache_disk_tier_miss", "count", ..t.prefix, 1_u64);
  ```

  With a spread present, literal pairs go to the dynamic
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
- **Kept:** the dispatch functions (`log_tagged`, `log_dyn_tagged`, `tagged_*_metric[_dyn]`)
  stay as the macros' expansion targets; `DynProperty` stays for lists built at runtime (the
  `tracing` interop, via `log_interop_dyn_tagged`).
- **Unchanged:** the SQL surface, the wire format, and the arena.
- **Reserved names.** Analytics treats some property names as column overrides:
  - logs (`analytics/src/log_entry.rs`, `TaggedLogString` arm): `target` and `level`;
  - metrics (`analytics/src/measure.rs`, tagged arms): `target`, `name` and `unit`.

  The property also stays in `properties`. `TaggedLogInteropEvent` doesn't apply overrides.
  Analytics matches these names case-sensitively, whereas `merge_properties` and
  `property_get` compare case-insensitively. Changes:
  - **Macros reject them.** For the log macros `level` and `target`, and for
    `imetric!`/`fmetric!` `target`, `name` and `unit`, as ident keys or string-literal keys,
    exact-token reserved-key arms in the key classifier that `$k:tt` is forwarded to expand to a
    `compile_error!` naming the real argument. This keeps `info!(target = x, ..)` from being
    confused with `info!(target: x, ..)`. The macros can't see inside a `..set` spread or a
    runtime `DynProperty` slice, so overrides through those still work; this is the documented
    escape hatch.
  - **Analytics keeps the override** (SQL and wire format are stable, and blocks already
    ingested rely on it).
- **Other notes:** the wasm stubs need the callsite-set helper. The `target:` metric arm also
  needs wasm-compatible expansion (it only changes the static metadata). On wasm32 the
  `property_set` stub has no `Property::new` / `find_or_create`, and `intern_string` /
  `static_string_ref` are native-only, and a closure passed to the helper is still type-checked
  there. The per-callsite `OnceLock<&'static PropertySet>` must be a `static` emitted by the
  macro expansion (a `static` inside a plain helper fn would be shared by all callsites). The
  expansion may name `PropertySet`, `OnceLock`, the helper, `PropertyValue`, `DynProperty` and the
  dispatch functions (the wasm32 stub defines `PropertySet` and `OnceLock::new()` is const).

## Implementation Steps

1. Per-callsite literal-set helper in `rust/tracing` (fills the macro-emitted per-callsite
   `static OnceLock<&'static PropertySet>` via `find_or_create`), plus the matching wasm32 stub
   next to the `property_set` stub in `rust/tracing/src/lib.rs` (~157-177). The helper dedups case-insensitively, last pair wins, as `merge_properties`
   does. The helper takes the
   `OnceLock` static and a `&'static [(&'static str, &'static str)]` (values via `concat!`), or a
   const-constructible struct holding both, so the macro never names `Property::new`,
   `find_or_create`, `intern_string` or `static_string_ref`.
2. Add the `PropertyValue` trait (with the explicit impls above) to `rust/tracing`, in a module
   compiled on both targets (it only needs `std::borrow::Cow`), not in the native-only
   `property_set.rs`.
3. Short-circuit an empty dynamic slice in the `Dispatch` methods in
   `rust/tracing/src/dispatch.rs`: `log_dyn_tagged` forwards to `log_tagged`, before its own
   enabled check and before `merge_properties`, so the sink check runs once instead of twice;
   `tagged_integer_metric_dyn` / `tagged_float_metric_dyn` forward to `tagged_integer_metric` /
   `tagged_float_metric`, before `merge_properties` and `get_or_insert`.
4. Rewrite `rust/tracing/src/macros.rs`: `log!`, `imetric!`, `fmetric!` and the level macros
   (kv muncher matching keys as `$k:tt =` with a key-classifying helper, trailing `..set` only,
   `PropertyValue` for bare values, `$k = - $v:literal` and the token-forwarding `-` fallback
   ahead of the `$v:literal` arm, the `log_enabled` gate for dynamic log entries, reserved-key
   `compile_error!` arms, `target:` arms feeding the muncher on logs and metrics).
   `LogMetadata.fmt_str` is taken from the format-string tokens the muncher separates out, not
   from the whole argument list (today's arms use `$crate::__first_arg!($($arg)+)`, which on
   `k = v, "msg"` returns `k`).
5. Migrate the ~76 callsites (see Constraints) to kv pairs or `..set`, move the `redis-exporter`
   sampler to kv pairs plus a trailing `..props`, and delete `log_json_rows.rs` (and the now-empty
   `utils` module).
6. Tests (below).
7. Docs and CHANGELOG (below).

### Tests
- Update `rust/tracing/tests/test_macros.rs` (`properties:` at 29-45, 4-argument metrics at
  86-103) to the new syntax. The calls at `:90` and `:99` pass `Property::new("name", ..)`;
  they keep `name` through a `..set` spread, as a deliberate test of the escape hatch
  (`name = ..` would hit the reserved-key `compile_error!`).
- Each kv form: literal, expr, `%`, `?`, string-literal key, `..set`, kv mixed with a trailing
  `..set` (kv overrides the set's entries).
- A bare `String`, `&str`, `Arc<String>` and `Cow` value compile and record the expected value;
  a bare `u32` fails to compile (doc-comment `compile_fail` example, as below).
- `..set` not in last position fails to compile with a clear `compile_error!`.
- Duplicate keys: within a bucket the last pair wins, including a case-insensitive duplicate;
  a dynamic value overrides a literal one with the same key in both orders
  (`k = x, k = "a"` and `k = "a", k = x` both record `x`); with a trailing `..set` the last
  pair wins.
- Trailing comma in log and metric lists.
- Pointer equality of the per-callsite literal set: with the `CapturingSink` + `DispatchGuard`
  pattern of `rust/tracing/tests/property_arena_tests.rs` (~187-250, and
  `static_path_does_not_touch_arena` ~310-337), compare the event's `properties` pointer for one
  callsite across iterations/flushes and assert the block arena stays empty for the literal
  path, for logs and for `imetric!`/`fmetric!` (literal-only and `..set`-only calls).
- Named format args after the format string.
- Metric `target:` arm, and `target:` combined with kv pairs on logs (`info!(target: "x", k = v,
  "msg")`) and metrics.
- A metric value starting with `-`: `fmetric!("n", "u", -x)` and `fmetric!("n", "u", k = v, -x)`;
  a kv value `k = -1` (literal, via `concat!`) and `k = %-x` (dynamic). A bare `k = -x` is a
  `compile_fail` case (the `Neg` output has no `PropertyValue` impl).
- A `#[wasm_bindgen_test]` in `rust/datafusion-wasm/tests/wasm_integration.rs` (compiled and run
  by `build.py --test` from `run_wasm()` in `build/rust_ci.py`) that uses the kv,
  trailing `..PropertySet::empty()` (the only `&'static PropertySet` on wasm32), a bare
  `&str`/`String` value, and metric `target:` arms under `wasm32-unknown-unknown`.
- Lazy evaluation: count evaluations of the value expression itself (a block
  `{ n += 1; s }`) for the bare, `%` and `?` forms. Disable the callsite with a test sink whose
  `on_log_enabled` returns false for a chosen target (e.g. a target filter on `CapturingSink`
  in `property_arena_tests.rs`): the counter stays 0; with the target enabled it is 1. A
  level-disabled case is only a sanity check, since the `max_level()` check already short-circuits it.
- No-property calls still emit untagged events.
- `compile_fail` doctests in `macros.rs` (none exist today) for reserved keys, a double
  `..set`, a bare `u32` value and a bare `k = -x`.

### Docs
- `rust/auth/src/db_api_key.rs:248` (doc comment showing the 4-argument `imetric!` form).
- `rust/tracing/src/macros.rs` doc comments (~163-180, ~239, ~308-322).
- `rust/tracing/README.md:25`.
- `rust/tracing/src/lib.rs:51-77` "# Dynamic properties" crate docs (shows
  `info!(dyn_properties: ..)`, the 5-argument `imetric!` and `properties:` prose). The
  "some property names override the row's columns" text must say override names now only
  arrive through `..set` or a runtime `DynProperty` slice.
- `rust/tracing/src/property_set.rs:27-33` (`DynProperty` doc), same override wording.
- `rust/redis-exporter/src/sampler.rs:5-8` module doc (values "interned to `'static`").
- `rust/object-cache/src/metric_tags.rs:1-9` module doc (class/prefix centralization).
- `rust/public/src/servers/query_audit.rs:5-7` (says `imetric!` sets can't carry
  high-cardinality values).
- `CHANGELOG.md:7-20` (Unreleased entry): rewrite for kv syntax, `..set`, the metric `target:`
  arm and reserved-name rejection, appending to the existing **Minor breaking change** clause
  at 14-20 (including the removed `properties:` / 4-argument forms, `class_tags` in `micromegas-object-cache` `metric_tags` and
  `rule_tags` in analytics, and the removed `micromegas::utils::log_json_rows` module, and the dropped `target` property on `payload_size_inserted`).

## Decisions
- `class_tags` is deleted; its callers use a dynamic `class = class` pair.
- The analytics invalid-`level` handling (`log_entry.rs`) is out of scope; to be done as its own change.
- Bare values go through a project-owned value trait with explicit impls (strings now; integer/bool impls can be added later without a break).
- `..set` must be last in the property list.
- `log_json_rows` is deleted rather than migrated: it has no callers.
- On enabled dynamic logs the sink-enabled check runs twice (the macro gate and `log_dyn_tagged`); this duplicate is accepted.
- No dedicated `compile_error!` arms for the removed `properties:` / `dyn_properties:` forms; the CHANGELOG breaking-change entry points users to `..set` and kv pairs.
