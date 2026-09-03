# crates.io Categories Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1565

## Overview

All 16 published Micromegas crates carry `keywords` but no `categories`, so none of them appear on
any crates.io category page or in lib.rs's category browse tree. This plan adds a shared
`categories` list to `[workspace.package]` in `rust/Cargo.toml`, opts every member crate into it
the same way they already opt into `keywords`, and overrides it on the handful of crates whose own
function is plainly outside the shared list.

## Current State

`rust/Cargo.toml:7-15` defines the shared package metadata:

```toml
[workspace.package]
version = "0.31.0"
edition = "2024"
license = "Apache-2.0"
homepage = "https://micromegas.info/"
documentation = "https://docs.rs/micromegas"
authors = ["Marc-Antoine Desroches <madesroches@gmail.com>"]
repository = "https://github.com/madesroches/micromegas/"
keywords = ["observability", "telemetry", "analytics"]
```

There is no `categories` key. Workspace package metadata is opt-in per crate, so 23 member
manifests each carry an explicit `keywords.workspace = true` line; adding `categories` to
`[workspace.package]` alone changes nothing until each crate opts in the same way.

Confirmed against the registry (`GET /api/v1/crates/micromegas`): `keywords` is
`["analytics", "telemetry", "observability"]`, `categories` is `[]`.

### Which crates are published

`build/release.py` is the authoritative list — 16 crates, matching what a crates.io search for
`micromegas` returns:

| Crate | Manifest |
|---|---|
| `micromegas-derive-transit` | `rust/transit/derive/Cargo.toml` |
| `micromegas-tracing-proc-macros` | `rust/tracing/proc-macros/Cargo.toml` |
| `micromegas-transit` | `rust/transit/Cargo.toml` |
| `micromegas-tracing` | `rust/tracing/Cargo.toml` |
| `micromegas-auth` | `rust/auth/Cargo.toml` |
| `micromegas-telemetry` | `rust/telemetry/Cargo.toml` |
| `micromegas-object-cache` | `rust/object-cache/Cargo.toml` |
| `micromegas-ingestion` | `rust/ingestion/Cargo.toml` |
| `micromegas-telemetry-sink` | `rust/telemetry-sink/Cargo.toml` |
| `micromegas-otel-ingestion` | `rust/otel-ingestion/Cargo.toml` |
| `micromegas-perfetto` | `rust/perfetto/Cargo.toml` |
| `micromegas-datafusion-extensions` | `rust/datafusion-extensions/Cargo.toml` |
| `micromegas-datafusion-wasm` | `rust/datafusion-wasm/Cargo.toml` |
| `micromegas-analytics` | `rust/analytics/Cargo.toml` |
| `micromegas-proc-macros` | `rust/micromegas-proc-macros/Cargo.toml` |
| `micromegas` | `rust/public/Cargo.toml` |

`micromegas-datafusion-wasm` is the one published crate that is **excluded** from the workspace
(`rust/Cargo.toml:3`), so it repeats every field literally (`rust/datafusion-wasm/Cargo.toml:2-12`)
and cannot use `.workspace = true`.

Eight further members inherit `keywords` but are never published (binaries):
`analytics-web-srv`, `flight-sql-srv`, `http-gateway`, `monolith`, `object-cache-srv`,
`redis-exporter`, `telemetry-ingestion-srv`, `telemetry-maintenance-srv`.

`rust/capi/Cargo.toml` and `rust/examples/write-perfetto/Cargo.toml` inherit no `keywords` today
and are not published — they stay untouched.

### Valid slugs

Every slug below was checked against the live registry
(`/api/v1/categories` and `/api/v1/categories/development-tools`); crates.io rejects a publish
carrying an unknown slug, and caps a crate at 5.

`development-tools::profiling`, `development-tools::debugging`,
`development-tools::procedural-macro-helpers`, `database-implementations`, `authentication`,
`caching`, `encoding`, `wasm`.

## Design

Add to `[workspace.package]`:

```toml
categories = ["development-tools::profiling", "development-tools::debugging", "database-implementations"]
```

These three are the shared default because they describe what the stack as a whole is: a profiler
and debugger's data source, backed by its own lakehouse implementation.

Add `categories.workspace = true` next to the existing `keywords.workspace = true` line in every
one of the 23 members that already inherits keywords — published or not. Following the keywords
precedent uniformly keeps one rule ("members inherit the shared package metadata") instead of a
second list of exceptions to maintain, and costs one line per manifest.

Override the default on five published crates whose own function falls outside it. An override
replaces the inherited list entirely — these manifests drop `categories.workspace = true` and
spell out a literal list instead:

| Crate | Categories | Why the default is wrong |
|---|---|---|
| `micromegas-derive-transit` | `["development-tools::procedural-macro-helpers", "encoding"]` | A derive macro for a serialization format, not a profiling or database crate |
| `micromegas-tracing-proc-macros` | `["development-tools::procedural-macro-helpers", "development-tools::profiling"]` | Instrumentation macros; keeps the profiling tie |
| `micromegas-proc-macros` | `["development-tools::procedural-macro-helpers"]` | Same |
| `micromegas-auth` | `["authentication"]` | API keys and OIDC; nothing profiling-, debugging-, or database-shaped |
| `micromegas-object-cache` | `["caching", "database-implementations"]` | A byte-range cache; `caching` is its primary identity |

`micromegas-transit` keeps the shared default rather than moving to `encoding`: it is the
low-overhead event serialization the tracing path is built on, and the profiling/debugging pages
are where someone would look for it.

`micromegas-datafusion-wasm` cannot inherit, so it gets a literal
`categories = ["wasm", "database-implementations", "development-tools::profiling"]` alongside its
existing literal `keywords` — `wasm` first because in-browser SQL is the distinguishing property.

## Implementation Steps

1. Add the `categories` key to `[workspace.package]` in `rust/Cargo.toml`, immediately after
   `keywords`.
2. Add `categories.workspace = true` directly below `keywords.workspace = true` in the 18
   manifests that take the shared default: `analytics`, `analytics-web-srv`,
   `datafusion-extensions`, `flight-sql-srv`, `http-gateway`, `ingestion`, `monolith`,
   `object-cache-srv`, `otel-ingestion`, `perfetto`, `public`, `redis-exporter`,
   `telemetry`, `telemetry-ingestion-srv`, `telemetry-maintenance-srv`, `telemetry-sink`,
   `tracing`, `transit`.
3. Add a literal `categories = [...]` line (per the override table) below `keywords.workspace = true`
   in the five overriding manifests: `transit/derive`, `tracing/proc-macros`,
   `micromegas-proc-macros`, `auth`, `object-cache`.
4. Add the literal `categories` line to `rust/datafusion-wasm/Cargo.toml`, below its literal
   `keywords`.
5. `CHANGELOG.md`: a bullet under `## Unreleased` recording the new shared `categories` metadata
   and the five per-crate overrides.
6. Run the verification below.

## Files to Modify

- `rust/Cargo.toml`
- `rust/{analytics,analytics-web-srv,auth,datafusion-extensions,datafusion-wasm,flight-sql-srv,http-gateway,ingestion,micromegas-proc-macros,monolith,object-cache,object-cache-srv,otel-ingestion,perfetto,public,redis-exporter,telemetry,telemetry-ingestion-srv,telemetry-maintenance-srv,telemetry-sink,tracing,transit}/Cargo.toml`
- `rust/tracing/proc-macros/Cargo.toml`
- `rust/transit/derive/Cargo.toml`
- `CHANGELOG.md`

## Trade-offs

- **Uniform opt-in vs. published-only opt-in.** Opting in only the 16 published crates would touch
  seven fewer files, but it would make `categories` the one shared field with a per-crate exception
  list that has to be kept in sync with `build/release.py` as crates come and go. Following the
  `keywords` precedent everywhere avoids that second list.
- **A CI lint asserting every published crate has non-empty categories** was considered and
  rejected. The failure it would catch — a future crate forgetting the opt-in line — costs a blank
  category page, not corrupted data or a broken build, and the check would need its own copy of the
  published-crate list to be worth anything.
- **Per-crate categories for the eight unpublished binaries** (e.g. `command-line-utilities`) buys
  nothing: they are never uploaded, so no category page ever reads the value.

## Documentation

None. `categories` is registry metadata with no user-facing documentation page in `mkdocs/`; the
change is recorded in `CHANGELOG.md` only.

## Testing Strategy

No automated test. The change is declarative manifest metadata with no reachable code path for a
unit test to call, and `cargo metadata` already resolves and reports the flattened value.

## Manual Verification

Not automated because the property being checked is what the registry will receive, which only
Cargo's own manifest resolution can answer.

1. From `rust/`, confirm every member resolved a non-empty list and that the overrides landed:

   ```
   cargo metadata --no-deps --format-version 1 \
     | python3 -c "import json,sys; [print(f\"{p['name']:38} {p['categories']}\") for p in sorted(json.load(sys.stdin)['packages'], key=lambda p: p['name'])]"
   ```

   Expect: `micromegas-capi` and `write-perfetto` empty; every other member either the shared
   three or its override row from the table above. A crate showing `[]` means its
   `categories.workspace = true` line is missing.

2. From `rust/datafusion-wasm/`, run the same command — it is a separate workspace, so step 1 does
   not cover it. Expect `["wasm", "database-implementations", "development-tools::profiling"]`.

3. Not automated because Cargo carries no category-slug list of its own — only crates.io validates
   slugs, and only at publish time. Confirm every slug used in the manifests is currently valid
   there (the crates.io API requires a descriptive `User-Agent` header, and `/api/v1/categories`
   needs `?per_page=100` to return all top-level slugs):

   ```
   curl -sS -A "micromegas-dev (madesroches@gmail.com)" \
     "https://crates.io/api/v1/categories?per_page=100" | grep -o '"slug":"[^"]*"'
   curl -sS -A "micromegas-dev (madesroches@gmail.com)" \
     "https://crates.io/api/v1/categories/development-tools" | grep -o '"slug":"[^"]*"'
   ```

   Expect `database-implementations`, `authentication`, `caching`, `encoding`, and `wasm` among the
   first command's output, and `development-tools::profiling`,
   `development-tools::debugging`, and `development-tools::procedural-macro-helpers` among the
   second's.

## Open Questions

None.
