# Sink Max Level from the Environment Plan

Issue: #1626

## Overview

`#[micromegas_main]` hardcodes the local (stdout) sink at DEBUG, and no binary can change that
without a rebuild. Where stdout is shipped to a log service billed per GB, that DEBUG console
copy is costly and redundant: the same events already reach micromegas through the telemetry
sink. This plan makes each built-in sink's max level configurable at startup through
environment variables read once by `TelemetryGuardBuilder::build()`. Changing a level while the
process is running is out of scope. That covers every
`#[micromegas_main]` binary and every direct builder user, including binaries built outside
this repo. It also lowers the macro's local-sink default from DEBUG to INFO. Along the way it
replaces the four copy-pasted `MICROMEGAS_TELEMETRY_*` env fallbacks with one shared
env-override helper.

## Current State

- `rust/micromegas-proc-macros/src/lib.rs:288-295`: the macro always emits
  `.with_local_sink_max_level(...)`, set to `LevelFilter::Debug` when the
  `local_sink_max_level` attribute is absent. The doc comment at line 38 says
  `(default: "debug")`. No binary in the repo sets the attribute, so every service's console
  runs at DEBUG.
- `rust/telemetry-sink/src/lib.rs:137-190`: the `TelemetryGuardBuilder` fields and their
  `Default`. `local_sink_max_level` already defaults to `LevelFilter::Info`, so only macro users
  get DEBUG. `telemetry_sink_max_level` is hardcoded to `LevelFilter::Debug` and has **no
  setter**.
- `rust/telemetry-sink/src/lib.rs:486-583`: `build()`. The env var pattern is repeated four
  times (lines 527-560):
  `self.x.or_else(|| std::env::var(VAR).ok().and_then(|v| v.parse().ok())).unwrap_or(DEFAULT)`.
  Precedence is explicit setter > env > default, and an unparseable value is **silently
  ignored**. `MICROMEGAS_PROCESS_PROPERTIES` (lines 492-494), by contrast, reads the variable
  with `var_os` and fails `build()` loudly on bad input.
- `rust/telemetry-sink/src/composite_event_sink.rs:28-39`: the global
  `micromegas_tracing::levels::set_max_level` is the max over all sink levels, unless
  `max_level_override` is set. `on_log` (lines 111-119) then filters each sink by its own
  level. So capping the local sink leaves the telemetry sink's DEBUG untouched. And when no
  telemetry URL is configured, it also lowers the global level, which makes `debug!` call sites
  near-free.
- `LevelFilter::from_str` (`rust/tracing/src/levels.rs:302`) already parses `off`, `fatal`,
  `error`, `warn`, `info`, `debug` and `trace`, case-insensitively.
- `rust/capi/src/lib.rs:98` disables the local sink, so only `MICROMEGAS_LOCAL_SINK_MAX_LEVEL`
  doesn't apply there. `mm_init` still calls `TelemetryGuardBuilder::build()`
  (`rust/capi/src/lib.rs:137-142`), so the new `MICROMEGAS_TELEMETRY_SINK_MAX_LEVEL` var and the
  now-strict transport parsing both reach it: on `Err`, `mm_init` `eprintln!`s and returns null,
  turning telemetry off, rather than panicking.
- `monolith` and `flight-sql-srv` pass `max_level_override = "debug"`, which pins the global
  level. Per-sink filtering in `on_log` still applies, so they honor the new cap too.

## Design

### 1. One env-override helper (generalization)

Add a private module `rust/telemetry-sink/src/env_config.rs` with the full surface used by
`build()`:

```rust
/// Pure core, unit-testable without touching the process environment.
fn parse_env_value<T: FromStr>(
    name: &str,
    raw: Option<OsString>,
    expected: &str,
) -> anyhow::Result<Option<T>>;

/// `parse_env_value(name, std::env::var_os(name), expected)`
pub(crate) fn env_override<T: FromStr>(name: &str, expected: &str) -> anyhow::Result<Option<T>>;

/// Pure core of `resolve`, unit-testable without touching the process environment.
fn resolve_from<T: FromStr>(
    explicit: Option<T>,
    name: &str,
    raw: Option<OsString>,
    default: T,
    expected: &str,
) -> anyhow::Result<T>;

/// `resolve_from(explicit, name, std::env::var_os(name), default, expected)`
pub(crate) fn resolve<T: FromStr>(
    explicit: Option<T>,
    name: &str,
    default: T,
    expected: &str,
) -> anyhow::Result<T>;
```

Semantics:

- Like `MICROMEGAS_PROCESS_PROPERTIES`, blank means unset and bad input fails loudly: if the
  variable is unset, or blank after trimming, the result is `Ok(None)`. A k8s manifest that
  renders an unset optional var produces `""`.
- Unlike `MICROMEGAS_PROCESS_PROPERTIES`, which lossily converts non-UTF-8 input, a non-UTF-8
  value here is an error.
- A value that fails `T::from_str` on the trimmed string is an error naming the variable and
  the value, e.g. `invalid MICROMEGAS_LOCAL_SINK_MAX_LEVEL "verbose": expected one of off,
  fatal, error, warn, info, debug, trace`. A generic helper cannot know the valid values of `T`,
  so `parse_env_value`, `env_override`, and `resolve` (and `resolve`'s pure core) each take an
  `expected: &str` parameter used only in that error message. Level call sites pass
  `"one of off, fatal, error, warn, info, debug, trace"`; `usize`/`u64` call sites pass e.g.
  `"a non-negative integer"`.

`build()` propagates these errors through `?`. The macro's `.expect(...)` already turns a
guard-build failure into a loud startup panic, so a typo fails at startup instead of being
silently ignored.

### 2. Level env vars

Add public consts next to `PROCESS_PROPERTIES_ENV_VAR`:

| Env var | Sink | Default |
|---|---|---|
| `MICROMEGAS_LOCAL_SINK_MAX_LEVEL` | local (stdout) | `info` |
| `MICROMEGAS_TELEMETRY_SINK_MAX_LEVEL` | HTTP telemetry | `debug` |

The telemetry-sink variable is the generalization. Both built-in sinks get the same knob, so an
operator can also trim what gets shipped to micromegas. The builder gains the missing
`with_telemetry_sink_max_level(LevelFilter)` setter to match `with_local_sink_max_level`.

**Precedence for levels: explicit builder call > env > default**, the same as the transport
knobs and process properties. A level set in code is a deliberate pin that the environment
cannot override. That includes the `local_sink_max_level` macro attribute, which is why the
macro must stop emitting the setter when the attribute is absent (§3).

Telling "explicitly set" apart from "default" needs the two level fields to become
`Option<LevelFilter>` (`None` in `Default`), with the defaults moved to consts
(`DEFAULT_LOCAL_SINK_MAX_LEVEL = Info`, `DEFAULT_TELEMETRY_SINK_MAX_LEVEL = Debug`). This
mirrors the existing `telemetry_max_queue_bytes: Option<usize>` fields. Resolution in `build()`:

```rust
let local_sink_max_level = resolve(
    self.local_sink_max_level,
    LOCAL_SINK_MAX_LEVEL_ENV_VAR,
    DEFAULT_LOCAL_SINK_MAX_LEVEL,
    "one of off, fatal, error, warn, info, debug, trace",
)?;
```

The same `explicit / env / default` shape then appears five times: two levels and three of the
four transport knobs (`telemetry_max_queue_bytes`, `telemetry_hard_queue_bytes`,
`telemetry_max_in_flight_requests`, all `Option<usize>`). `resolve` covers those five.

Resolve both levels and all four transport knobs at the top of `build()`, next to the
process-properties merge, and use the resolved values inside the `if let Some(url)` branch (§4).
That way a bad value fails before the global guard is created. It also fails even when that sink
ends up unused, for example `MICROMEGAS_TELEMETRY_SINK_MAX_LEVEL` or
`MICROMEGAS_TELEMETRY_MAX_QUEUE_BYTES` without a URL. A typo should not hide until the URL is
set.

`add_sink` extra sinks, `max_level_override`, and `interop_max_level` are not given env vars.
Extra sinks are caller-owned. The other two knobs are about capture cost, not output volume, and
nobody has asked for them.

### 3. Macro default: INFO

In `expand_micromegas_main`, emit `.with_local_sink_max_level(...)` **only when the attribute
is present**. With no attribute, the builder's own `Info` default applies. This removes the
macro's second copy of the default, so the builder is the single source of truth. Update the
doc comment to `(default: "info")` and mention that `MICROMEGAS_LOCAL_SINK_MAX_LEVEL`
overrides it.

### 4. Migrate the transport env fallbacks to the helper

Replace three of the four inline `or_else(|| std::env::var(...))` blocks with the shared
`resolve` helper from §2, e.g.
`resolve(self.telemetry_max_queue_bytes, MAX_QUEUE_BYTES_ENV_VAR, HttpSinkConfig::DEFAULT_MAX_QUEUE_BYTES, "a non-negative integer")?`.
The fourth, the request timeout, can't use `resolve` because `telemetry_request_timeout` is
`Option<Duration>` and `Duration` isn't `FromStr`; it stays a `match` on `env_override::<u64>`:
`match self.telemetry_request_timeout { Some(d) => d, None =>
env_override::<u64>(REQUEST_TIMEOUT_SECS_ENV_VAR, "a non-negative integer")?.map(Duration::from_secs).unwrap_or(HttpSinkConfig::DEFAULT_REQUEST_TIMEOUT) }`.
Their precedence (explicit > env) is unchanged. The only behavior change is that an unparseable
value such as `MICROMEGAS_TELEMETRY_MAX_QUEUE_BYTES=128MiB` now fails startup instead of being
silently replaced by the default.

`MICROMEGAS_ENABLE_CPU_TRACING` stays as is. It uses `== "true"` semantics, where `"1"` means
off, and moving it to `bool::from_str` would turn today's silent-false values into startup
failures, with no request driving that change.

## Implementation Steps

1. Create `rust/telemetry-sink/src/env_config.rs` with `parse_env_value` / `env_override` /
   `resolve` (and `resolve`'s pure `Option<OsString>` core) and unit tests. Register it in
   `lib.rs` as a native-only private module.
2. In `rust/telemetry-sink/src/lib.rs`:
   - add `LOCAL_SINK_MAX_LEVEL_ENV_VAR` / `TELEMETRY_SINK_MAX_LEVEL_ENV_VAR` consts (doc
     comments state the precedence and accepted values);
   - change `local_sink_max_level` / `telemetry_sink_max_level` to `Option<LevelFilter>` with
     `DEFAULT_*_MAX_LEVEL` consts;
   - add `with_telemetry_sink_max_level`;
   - resolve both levels and the four transport knobs at the top of `build()` through
     `resolve` (the request timeout through a direct `env_override::<u64>` call, mapped to a
     `Duration`), promoting the transport env var names to consts, and use the resolved values
     where the sinks are pushed (lines ~566 and ~580); update the setter doc comments to say
     invalid values fail `build()`.
3. In `rust/micromegas-proc-macros/src/lib.rs`: emit `with_local_sink_max_level` only when the
   attribute is present, update the doc comment (also fixing the existing `local_sink_enabled`
   doc line, which says "enable local stderr sink" but the sink writes to stdout), update line
   14's "Telemetry guard with sensible defaults (ctrl-c handling, debug level)" so "debug level"
   no longer goes stale (e.g. "info console level, overridable via
   `MICROMEGAS_LOCAL_SINK_MAX_LEVEL`"), and update the tests (see Testing).
4. Update the docs and `CHANGELOG.md` (see Documentation).

## Files to Modify

- `rust/telemetry-sink/src/env_config.rs` (new)
- `rust/telemetry-sink/src/lib.rs`
- `rust/micromegas-proc-macros/src/lib.rs`
- `mkdocs/docs/admin/telemetry-sink-tuning.md`
- `CHANGELOG.md`

## Trade-offs

- **Per-sink vars vs. one `MICROMEGAS_CONSOLE_LEVEL`.** Naming the vars after the builder
  fields generalizes to both built-in sinks with one pattern, and makes the mapping to the
  Rust API obvious.
- **Reusing the dead `target_max_levels` for per-target levels (e.g. silence
  `lakehouse::write_partition`).** Rejected. `CompositeSink` applies a target level
  *instead of* each sink's level (`target_max_level.unwrap_or(max_level)`), so a target rule
  would override the local cap rather than narrow it. A per-sink cap solves the issue without
  reworking that filter. The field has never been populated since the initial import and could
  be removed separately.
- **Parsing levels in the proc macro with `LevelFilter::from_str`.** Not done. The macro's
  `level_to_filter` must emit tokens and report compile-time errors with spans, so sharing
  code with the runtime parser buys little.

## Decisions

- Levels are set once at startup through env vars, and are not reloaded while the process runs
  (user call).
- The macro's default local-sink level becomes INFO. The issue asked for unset to keep DEBUG;
  the user overrode that.
- The existing `MICROMEGAS_TELEMETRY_*` transport vars move to strict parsing (Design §4). An
  invalid value fails startup instead of being silently ignored (user call).
- Precedence for every builder env var, levels included, is default < env < explicit call in
  code (user call).

## Documentation

- `mkdocs/docs/admin/telemetry-sink-tuning.md`: new "Log levels" section documenting both
  variables, accepted values, defaults, precedence (a level set in code or through the macro
  attribute cannot be overridden), and the cost motivation: capping
  the console to `info` while keeping `debug` in telemetry. Note that invalid transport values
  now fail startup, and that the C ABI (`mm_init`) returns null instead of panicking on an
  invalid `MICROMEGAS_TELEMETRY_SINK_MAX_LEVEL` or transport var; `MICROMEGAS_LOCAL_SINK_MAX_LEVEL`
  doesn't apply there because its local sink is always disabled.
- `rust/micromegas-proc-macros/src/lib.rs`: the macro doc comment (default and env override).
- `CHANGELOG.md` Unreleased entry: the new env vars and setter, the default console level for
  `#[micromegas_main]` binaries dropping from DEBUG to INFO (a visible behavior change;
  set `MICROMEGAS_LOCAL_SINK_MAX_LEVEL=debug` to restore it), strict parsing of the
  `MICROMEGAS_TELEMETRY_*` transport vars, and that `mm_init` now returns null instead of
  initializing telemetry when `MICROMEGAS_TELEMETRY_SINK_MAX_LEVEL` or a transport var is
  invalid.

## Testing Strategy

Unit tests only. No live DB is involved.

- `env_config.rs`, on `parse_env_value`, so the tests never mutate the process env:
  - unset → `None`;
  - `""` and `"  "` → `None`;
  - `"INFO"`, `" debug "` → parsed `LevelFilter`;
  - `"verbose"` → error whose message contains the variable name and the value;
  - non-UTF-8 `OsString` (Unix `OsStringExt::from_vec`, `#[cfg(unix)]`) → error;
  - `"123"` / `"12x"` as `usize` → `Some(123)` / error.
- Precedence: give `resolve` the same pure core as `parse_env_value`, taking the raw
  `Option<OsString>` instead of reading the env. Assert that explicit beats a set env value,
  that env beats the default, that the default is used when both are absent, and that an
  invalid env value is an error only when nothing explicit is set. An explicit value means the
  env value is never parsed. `build()` itself installs a process-global guard and is not
  unit-testable in isolation.
- `micromegas-proc-macros` tests:
  - change the existing assertion at line 386, so that no attribute →
    `with_local_sink_max_level` is **absent** from the expansion;
  - keep `local_sink_max_level_custom_emits_correct_filter` (attribute → call emitted with the
    right filter).

## Manual Verification

These steps check the wiring from env var through `build()` to real stdout output, which unit
tests cannot reach, and a breakage would be obvious on the next run.

1. `python3 local_test_env/ai_scripts/start_services.py`, then
   `grep -c " DEBUG " /tmp/daemon.log`. Expect `0`: no DEBUG lines on the maintenance daemon
   console at the default.
2. `micromegas-query "SELECT count(*) FROM log_entries WHERE level = 5" --begin 10m`.
   Expect a non-zero count, which shows the telemetry sink still receives DEBUG.
3. Stop the services, restart with `MICROMEGAS_LOCAL_SINK_MAX_LEVEL=debug` exported, and
   check `/tmp/daemon.log`. Expect DEBUG lines again.
4. Restart with `MICROMEGAS_LOCAL_SINK_MAX_LEVEL=verbose`. Expect the daemon to exit at
   startup with `failed to initialize micromegas telemetry` and the variable named in the error
   chain.
