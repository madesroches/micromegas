# Deferred Log Message Formatting Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1653

## Overview

In `info!(user = %user, "login for {user}")`, the process formats `"login for alice"`, allocates
a `String`, and copies the message bytes into the log block, even though `user` already travels
as a property. This plan adds a tagged log event that carries no message: it sends the template
(`desc.fmt_str`) and the property set, and analytics renders `msg` when it parses the block. The
issue measured formatting at about 35% of a low-cardinality call, and the formatted event at
about 3-4x the size of a deferred one.

A proc macro builds the log calls. It parses the format string at compile time, binds every
`{key}` placeholder to the property's value, and picks the event kind (static, template or
formatted). Because of that binding, what the process prints and what analytics renders come
from the same value, and nothing about the message is checked at runtime. The SQL surface
doesn't change: `msg` still holds the rendered text.

## Current State

**Macros** (`rust/tracing/src/macros.rs`):

- `__props!` (`:674`) sorts the entries into three lists: literal/static pairs
  `(key, lit|stat, (value))`, dynamic entries `(key, bare|disp|dbg, value)`, and every
  non-static entry in source order. Its `@end log` arms (`:742`-`:790`) pick a shape (no
  properties, `..set`, callsite set, static set, any of these plus dynamic values) and call
  `__log_call!(plain|nogate|gate ...)` (`:809`).
- `__log_call!` builds `LOG_DESC` (with `fmt_str: __first_arg!(fmt)`), gates on the level (and,
  for `gate`, on `enabled_log_sink`), and calls `dispatch::log`, `log_dyn_tagged` or
  `log_dyn_tagged_enabled` with `format_args!(fmt)`.
- `disp` values become `format!("{}", v)`. `bare` values go through `PropertyValue`, which is
  implemented only for string types (`rust/tracing/src/property_value.rs`), where it equals
  `Display`. Static values are evaluated inside `__static_set!`'s `CACHE.get(...)` (`:947`).
- A `{name}` placeholder captures whatever identifier `name` resolves to at the call site, not
  the property. In `info!(user = %account.name, "login for {user}")`, the message uses the local
  `user`, if there is one.

**Dispatch** (`rust/tracing/src/dispatch.rs`): `log` (`:937`), `log_tagged_to_sink` (`:979`) and
`log_dyn_tagged_to_sink` (`:1059`) decide the event at runtime with `is_static_msg` (`:595`,
`args.as_str() == Some(desc.fmt_str)`). They push the static event (`LogStaticStrEvent`,
`TaggedLogStaticStrEvent`) if it holds, and otherwise the formatted one (`LogStringEvent`,
`TaggedLogString`, with `args.to_string()`). Sinks receive `fmt::Arguments`. Only the console
and local sinks format it. With the HTTP sink, `args.to_string()` in dispatch is the only
formatting on the calling thread. The `dispatch::log*` entry points are only called by the
macros.

**Proc macros** (`rust/tracing/proc-macros`): `span_fn` and `log_fn` attributes, built on
`syn` (`full`), `quote` and `proc-macro2`. They are re-exported through
`micromegas_tracing::prelude` (`rust/tracing/src/lib.rs:303`).

**Events** (`rust/tracing/src/logs/log_events.rs`): `TaggedLogStaticStrEvent { desc, properties,
time }` is a derived POD whose message is `desc.fmt_str` verbatim. `TaggedLogString` adds a
`DynString` `msg`. `LogMsgQueue` and `extract` are in `rust/tracing/src/logs/block.rs`.

**Analytics** (`rust/analytics/src/log_entry.rs`): `log_entry_from_value` dispatches on the type
name. The `TaggedLogString` and `TaggedLogStaticStrEvent` arms both call `tagged_log_entry`
(`:240`) with a `msg: &'a str` borrowed from the parse arena. `LogEntry::msg` is `&'a str`, and
its only consumer is `LogEntriesRecordBuilder` (`rust/analytics/src/log_entries_table.rs:166,179`).
An unknown type name logs a warning and the entry is dropped.

**Precedent:** `tasks/completed/tagged_log_static_msg_plan.md` added `TaggedLogStaticStrEvent`
the same way (derived POD event, shared analytics helper, analytics upgraded first).

## Design

### Placeholder semantics

In a log call with properties, a placeholder whose argument name is a property key refers to
that property's value: the value expression, evaluated once. It no longer refers to a local with
the same name. Every other placeholder (`{n}` with no key `n`, `{}`, `{0}`) keeps today's
meaning.

- `info!(user = %account.name, "login for {user}")` prints and stores `account.name`.
- `info!(n = %n, "{n:.2}")` formats the original `n`, not its `Display` string. Specs keep
  working, so the output doesn't change.
- A key that exists only in a `..set` spread is unknown at compile time and isn't bound.

A scan of `rust/` found no non-test call site that uses properties together with a named
placeholder, so no existing message changes.

### Compile-time decisions

The proc macro decides all of the following when the call expands. At runtime, only level and
sink gating, property merging and interning, and analytics rendering remain.

| Check | Outcome |
|---|---|
| Format string is not a literal (`concat!(..)`) | No binding; `Formatted` |
| Literal with no `{` or `}` and no extra arguments | `Static` |
| Placeholder name matches two keys case-insensitively | `compile_error!`, since the binding would be ambiguous |
| Placeholder name is also passed explicitly (`"{user}", user = x`) | Not bound (the explicit argument wins); `Formatted` |
| Every placeholder is `{k}` with a `%`/bare/literal/static key, or `{k:?}` with a `?` key, and there are no extra arguments | `Template` (escapes allowed, zero placeholders allowed) |
| Anything else (specs, positional, unbound names, extra arguments) | Bind what matches a key; `Formatted` |

The pairing rule exists because a deferred placeholder renders as the stored property string.
`{k}` renders `Display`, which equals the stored string for `%`, bare (string `PropertyValue`),
literal and static values. `{k:?}` renders `Debug`, which equals it for `?` values. A `{k}` over a
`?` key is still bound, but formatted.

The previous plan fixed a bug where `is_static_msg` stored `{}` for `info!("{}", "{}")`. The
compile-time `Static` rule keeps that fix: any brace or extra argument rules a call out of
`Static`. `is_static_msg` goes away. Non-literal format strings lose the runtime static check,
which is rare and still correct.

### Proc macro `__log_impl!`

It lives in `rust/tracing/proc-macros/src/lib.rs` (format scanning in a new
`src/log_impl.rs`), is `#[doc(hidden)]`, and is re-exported at the `micromegas_tracing` root so
that the `#[macro_export]` macros can reach it. It replaces `__log_call!` and the body of
`__props!`' `@end log` arms. `__props!` keeps parsing the property syntax, so the reserved-key
errors and the existing `compile_fail` doctests stay. The metric arms don't change.

Input, from the `@end log` arms: `$crate` (proc macros can't name it themselves), the
`[target, lvl]` context, the muncher's accumulators (literal/static pairs, dynamic entries,
source-order entries, spread set, static marker), and the format tokens.

Output, for an enabled entry:

```rust
static LOG_DESC: LogMetadata = { .. fmt_str: <first fmt token> .. };
if <level gate> {
    if let Some(sink) = enabled_log_sink(&LOG_DESC) {        // only when values are evaluated
        let __mm_v_user = &(account.name);                   // each dyn/static value, once
        let __mm_p_user = format!("{}", __mm_v_user);        // disp; dbg uses {:?}, bare PropertyValue
        dispatch::log_dyn_tagged_enabled(
            sink, &LOG_DESC, <set>, &[DynProperty::new("user", &*__mm_p_user)],
            LogMsgForm::Template,
            format_args!("login for {user}", user = __mm_v_user),
        );
    }
}
```

- Hidden locals use `Span::mixed_site()` so they can't collide with user identifiers. Static
  values are hoisted the same way and passed to the static-set cache, so they're evaluated once.
- Only names found in argument position are passed as named arguments. The scanner reads the
  text before the first `:` of each `{..}`, and leaves `name$` width and precision references to
  inline capture. A scanning mistake shows up as rustc's "named argument never used" or
  "duplicate argument" error, never as a wrong message.
- The shape selection (`plain`, `nogate`, `gate`, callsite set, static set) is ported from the
  `@end log` arms. The generated code reuses `__callsite_set!`, `__static_set!` and
  `__dyn_prop!` through the passed `$crate`.

### Dispatch

The `dispatch::log*` entry points and `Dispatch::log_tagged_to_sink` /
`log_dyn_tagged_to_sink` take `form: LogMsgForm`:

```rust
pub enum LogMsgForm { Static, Template, Formatted }
```

| form | untagged (`log`) | tagged |
|---|---|---|
| `Static` | `LogStaticStrEvent` | `TaggedLogStaticStrEvent` |
| `Template` | not emitted by the macro; treated as `Formatted` | `TaggedLogTemplateEvent` |
| `Formatted` | `LogStringEvent` | `TaggedLogString` (`args.to_string()`) |

`on_log` is unchanged, so sinks still get `args`. `log_tagged_to_sink` needs the `Template` arm
too: a call with only literal or static keys (`info!(zone = "eu", "in {zone}")`) defers with its
interned set.

### New event

```rust
/// A log whose message is `desc.fmt_str` rendered with `properties`.
#[derive(Debug, TransitReflect)]
pub struct TaggedLogTemplateEvent {
    pub desc: &'static LogMetadata<'static>,
    #[transit(reference)]
    pub properties: PropertySetId,
    pub time: i64,
}
impl InProcSerialize for TaggedLogTemplateEvent {}
```

It has the same layout as `TaggedLogStaticStrEvent`, but it's a separate type name: stored
static events mean "verbatim" and must not be re-rendered. Add it to `LogMsgQueue`, export it
from `logs`, and give it an `extract` arm identical to `TaggedLogStaticStrEvent`'s. No custom
reader is needed.

### Analytics

- `LogEntry::msg` becomes `Cow<'a, str>`. The existing arms pass `Cow::Borrowed`, and
  `LogEntriesRecordBuilder` appends `&*row.msg`. `tagged_log_entry` takes the `Cow`.
- New `"TaggedLogTemplateEvent"` arm: read `desc.fmt_str` and `properties`, and call
  `tagged_log_entry` with `Cow::Owned(render_msg_template(fmt_str, properties))`.
- `pub fn render_msg_template(fmt_str: &str, properties: &Object) -> String`, in a new
  `rust/analytics/src/log_msg_template.rs`. It handles the grammar the macro defers: text,
  `{{`/`}}`, `{name}` and `{name:?}`, where the last two are replaced with the `Value::String`
  member named `name`.
- A missing property, or a placeholder outside that grammar, is copied through verbatim instead
  of failing the block. This producer never emits either; the fallback only keeps a corrupt or
  hand-built block readable.
- Rendering happens where blocks are parsed: when log partitions are materialized, and for JIT
  process views. It doesn't run per query over materialized data.

### Wire compatibility

The event is a new type name. An analytics service older than this change parses the block, but
each `TaggedLogTemplateEvent` hits the `_` arm: a warning is logged and the entry is dropped.
**Analytics must be upgraded before instrumented processes**, as with `TaggedLogStaticStrEvent`.
Unreal instrumentation and the `tracing` interop layer don't emit the new event. `tracing`
doesn't expose the message template, so the interop path can't benefit.

## Implementation Steps

### Phase 0: Benchmark baseline

1. `rust/tracing/benches/properties.rs`: add log cases that put a property in the message:
   - `dyn_low_card_msg`: `let l = LABELS[i % 4]; info!(l = l, "label {l}")`;
   - `dyn_high_card_msg`: `info!(id = %i, "request {id}")`;
   - `static_low_card_msg`: `info!(l = static LABELS[i % 4], "label {l}")`.

   Add all three to the `extract` group. Before the change, these calls format in the process.
   (`dyn_high_card_msg` compiles today only if a local `id` exists, so bind `let id = i;`.)
2. Commit, then from `rust/`:
   `cargo bench -p micromegas-tracing --bench properties --bench with_dispatch -- --save-baseline before`.
   Record the "Before" column. Don't re-save the baseline after making changes.

### Phase 1: Event and dispatch

3. `rust/tracing/src/logs/log_events.rs`: add `TaggedLogTemplateEvent`, and add `LogMsgForm` in
   `rust/tracing/src/logs/events.rs`. Export both from `logs`.
4. `rust/tracing/src/logs/block.rs`: add the event to `LogMsgQueue` and `extract`.
5. `rust/tracing/src/dispatch.rs`: add the `form` parameter to `log`, `log_tagged`,
   `log_dyn_tagged`, `log_dyn_tagged_enabled` and the `Dispatch` methods, and remove
   `is_static_msg`.

### Phase 2: Proc macro

6. `rust/tracing/proc-macros/src/log_impl.rs`: format-string scanner, binding resolution, form
   selection, and code generation. Register `__log_impl` in `lib.rs`.
7. `rust/tracing/src/lib.rs`: re-export `__log_impl` at the crate root (`#[doc(hidden)]`).
8. `rust/tracing/src/macros.rs`: route the `@end log` arms of `__props!` to `__log_impl!`, and
   delete `__log_call!`.
9. Update the in-process readers of `LogMsgQueueAny`: `rust/tracing/tests/utils.rs` (exhaustive
   match), `property_macro_tests.rs` (record the new kind), and `stream_flush_tests.rs` (read
   `properties` from the new event too).

### Phase 3: Analytics

10. `rust/analytics/src/log_msg_template.rs` (new): `render_msg_template`.
11. `rust/analytics/src/log_entry.rs`: make `msg` a `Cow` and add the new arm.
    `rust/analytics/src/log_entries_table.rs`: append `&*row.msg`.

### Phase 4: Tests

12. Add the tests in Testing Strategy.

### Phase 5: Measure and document

13. From `rust/`: `cargo bench -p micromegas-tracing --bench properties --bench with_dispatch -- --baseline before`.
    Fill the "After" column. Re-run any case that moves by more than noise for a reason this
    change doesn't explain.
14. Rustdoc and `CHANGELOG.md` (see Documentation).

## Files to Modify

- `rust/tracing/proc-macros/src/lib.rs`, `rust/tracing/proc-macros/src/log_impl.rs` (new)
- `rust/tracing/src/macros.rs`, `rust/tracing/src/lib.rs`
- `rust/tracing/src/logs/log_events.rs`, `rust/tracing/src/logs/events.rs`,
  `rust/tracing/src/logs/block.rs`, `rust/tracing/src/logs/mod.rs`
- `rust/tracing/src/dispatch.rs`
- `rust/tracing/benches/properties.rs`
- `rust/tracing/tests/utils.rs`, `property_macro_tests.rs`, `stream_flush_tests.rs`
- `rust/analytics/src/log_msg_template.rs` (new), `rust/analytics/src/lib.rs`,
  `rust/analytics/src/log_entry.rs`, `rust/analytics/src/log_entries_table.rs`
- `rust/analytics/tests/log_tests.rs`, `rust/analytics/tests/log_msg_template_tests.rs` (new)
- `CHANGELOG.md`

## Trade-offs

- **Proc macro vs. a runtime check per callsite in `macro_rules`.** `macro_rules` can't look
  inside the format string, so the alternative was a cached runtime check that compared
  `stringify!(value)` with the key, and covered only `key = %key`. That left a placeholder
  meaning a same-named local, and excluded literal and static values and every
  `key = %expr` form. The proc macro binds the placeholder to the property, so the stored and
  printed messages can't diverge, and it costs nothing at runtime.
- **Proc macro for the log tail only, not the whole `log!`.** `__props!` already parses the
  property syntax and owns the reserved-key and `..set` errors and their `compile_fail` tests.
  Moving only the code generation keeps that tested surface intact.
- **Separate format grammars in the proc macro and analytics.** A proc-macro crate can only
  export macros, so the two can't share a parser without a new crate. The analytics grammar is
  small (text, escapes, `{name}`, `{name:?}`), and the round-trip test (macro, then analytics
  render, compared with `format!`) catches any disagreement.
- **Reusing `TaggedLogStaticStrEvent`.** Rejected because stored static events would be
  re-rendered.
- **No `template` column.** Nothing asks for it. Appending it last later is an additive SQL
  change.
- **`Cow` in `LogEntry` vs. exposing the parse arena to the callback.** `Cow` keeps
  `parse_block`'s signature unchanged and costs one allocation per rendered row at
  materialization.

## Decisions

- Generate log calls with a proc macro that binds `{key}` placeholders to properties and picks
  the event kind at compile time, instead of a runtime per-callsite check (user call,
  2026-10-19).
- A placeholder that matches two keys case-insensitively is a compile error rather than
  following the merge's "last wins" rule.
- Case-insensitive property names are inherited behavior, not a hard requirement. If supporting
  them adds real complexity, switch the tracing crate to case-sensitive names instead, even
  though that changes current behavior (user call, 2026-10-19).

## Documentation

- `log!` rustdoc (`rust/tracing/src/macros.rs`): the placeholder semantics; which calls are
  stored as a template (the pairing rule) and why that's the cheapest way to put a property in
  the message; a `compile_fail` doctest for the ambiguous-key error.
- Crate docs (`rust/tracing/src/lib.rs:72`): the example
  `rows = %n, "fetched {n} rows"` becomes `"fetched {rows} rows"`, the form that defers.
- No `mkdocs/` page documents log macro properties or event types.
- `CHANGELOG.md`, in the style of the recent tracing performance entries:
  - the new event, and that analytics must be upgraded before instrumented processes;
  - the placeholder semantics change;
  - the before/after medians;
  - **Minor breaking change:** `LogEntry::msg` is `Cow<'a, str>`, the `dispatch::log*`
    functions take a `LogMsgForm`, `LogMsgQueue` gains a variant (exhaustive matches on
    `LogMsgQueueAny` must add it), and a placeholder naming two keys is a compile error;
  - no SQL change.

## Testing Strategy

No-DB unit tests and doctests only. Nothing here reproduces a bug seen in the wild.

**`rust/tracing/tests/property_macro_tests.rs`** (event kind per call, through the capturing
sink). Each call also checks that the sink's `on_log` arguments format to the expected message.

- `TaggedLogTemplateEvent`:
  - `info!(user = %user, "login for {user}")`;
  - `info!(user = user, ..)` with a `&str`;
  - `info!(user = %user, zone = %zone, "{user} in {zone}")`;
  - `info!(zone = "eu", "in {zone}")` (interned set);
  - `info!(l = static label, "label {l}")`;
  - `info!(v = ?v, "{v:?}")`;
  - `info!(a = "1", "a {{b}}")` (escapes only).
- Binding:
  - `info!(user = %account.name, "{user}")` with a different local `user` in scope prints
    `account.name`, and pushes the template event;
  - `info!(id = %next(), "{id}")` evaluates `next()` once.
- `TaggedLogString` with the printed message:
  - `info!(n = %n, "{n:>8}")`, which formats the original `n`;
  - `info!(v = ?v, "{v}")`;
  - `info!(user = %user, "{user} {}", 3)`;
  - `info!(a = "1", "{}", "{}")`;
  - `info!(..set, "{region}")` with a local `region`;
  - `info!(user = %user, "{user}", user = other)`.
- `TaggedLogStaticStrEvent`: `info!(user = %user, "static")`.
- Untagged: `info!("static")` is `LogStaticStrEvent`; `info!("a {{b}}")` and
  `info!("value {}", 3)` are `LogStringEvent`.
- Disabled entries: a gated call at a filtered level doesn't evaluate dynamic or static values
  (counter in the value expression).

**Doctests in `macros.rs`**: `compile_fail` for `info!(user = %a, User = %b, "{user}")`.

**`rust/analytics/tests/log_msg_template_tests.rs`**: `render_msg_template` on text, escapes,
adjacent and edge placeholders, `{name:?}`, a missing property (kept verbatim), and an
unsupported placeholder (kept verbatim).

**`rust/analytics/tests/log_tests.rs`**:

- A round trip through the real macros: `init_in_memory_tracing`, then the template-event calls
  above, then `parse_entries`. Assert that each `msg` equals the `format!` of the same template
  and values. This pins agreement between the proc macro's grammar and the analytics renderer.
- A `level`/`target` override property on a template event still applies through
  `tagged_log_entry`.

The existing `dyn_deps`, stream-flush, analytics log and corrupt-block suites cover the rest of
the extract and parse plumbing once they're updated.

## Benchmarks

Command: `cargo bench -p micromegas-tracing --bench properties --bench with_dispatch`, from
`rust/`. Medians. "Before" is the Phase 0 baseline.

| Case | Before | After |
|---|---|---|
| log/dyn_low_card_msg | | |
| log/dyn_high_card_msg | | |
| log/static_low_card_msg | | |
| log/static, log/formatted | | |
| log/dyn_low_card, log/dyn_high_card | | |
| extract/log/dyn_low_card_msg | | |
| extract/log/dyn_high_card_msg | | |
| dispatch/log | | |

Expected:

- `dyn_low_card_msg` and `static_low_card_msg` drop by about a third (the issue measured +28 ns
  of formatting on an ~81 ns call), and their bytes per event fall to those of the
  corresponding `*_low_card` cases.
- `dyn_high_card_msg` should be at or slightly below its baseline, because interning dominates.
- `log/static` and `log/formatted` may improve slightly, since the runtime `is_static_msg`
  comparison is gone.
- The other cases are controls.
