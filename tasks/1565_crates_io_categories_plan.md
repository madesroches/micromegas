# crates.io Categories Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1565

## Overview

All 16 published Micromegas crates carry `keywords` but no `categories`, so none of them appear on
any crates.io category page or in lib.rs's category browse tree. This plan adds a shared
`categories` list to `[workspace.package]` in `rust/Cargo.toml`, opts every member crate into it
the same way they already opt into `keywords`, and overrides that default on a handful of crates —
some add to the shared pair, others replace it outright.

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
(`/api/v1/categories` and `/api/v1/categories/development-tools`); crates.io silently drops an
unknown slug with a publish-time warning rather than failing the publish, and caps a crate at 5.

`development-tools::profiling`, `development-tools::debugging`,
`development-tools::procedural-macro-helpers`, `database-implementations`, `authentication`,
`caching`, `encoding`, `wasm`.

## Design

Add to `[workspace.package]`:

```toml
categories = ["development-tools::profiling", "development-tools::debugging"]
```

These two are the shared default; most crates here take them as-is. The override table below
either adds to that pair (the lakehouse crates also gain `database-implementations`) or replaces
it outright (auth, cache, and macro crates have no profiling/debugging role).

Add `categories.workspace = true` next to the existing `keywords.workspace = true` line in the 12
members that take the shared default, published or not.

The remaining 11 members omit `categories.workspace = true` and spell out a literal list instead —
Cargo has no per-field merge, so an override states the whole list:

| Crate | Categories | Why not the default |
|---|---|---|
| `micromegas`, `micromegas-analytics`, `micromegas-ingestion`, `micromegas-otel-ingestion`, `micromegas-datafusion-extensions` | `["development-tools::profiling", "development-tools::debugging", "database-implementations"]` | The lakehouse itself — storage, write path, and query engine |
| `micromegas-transit` | `["development-tools::profiling", "development-tools::debugging", "encoding"]` | Low-overhead serialization for the tracing path; keeps the profiling/debugging tie |
| `micromegas-derive-transit` | `["development-tools::procedural-macro-helpers", "encoding"]` | A derive macro for a serialization format |
| `micromegas-tracing-proc-macros` | `["development-tools::procedural-macro-helpers", "development-tools::profiling"]` | Instrumentation macros; keeps the profiling tie |
| `micromegas-proc-macros` | `["development-tools::procedural-macro-helpers", "development-tools::profiling"]` | Same |
| `micromegas-auth` | `["authentication"]` | API keys and OIDC; nothing profiling- or debugging-shaped |
| `micromegas-object-cache` | `["caching"]` | A byte-range cache, not a database |

`micromegas-datafusion-wasm` cannot inherit, so it gets a literal
`categories = ["wasm", "database-implementations", "development-tools::profiling", "development-tools::debugging"]`
alongside its existing literal `keywords` — `wasm` first because in-browser SQL is the
distinguishing property, and the rest match the other lakehouse crates.

## Implementation Steps

1. Add the `categories` key to `[workspace.package]` in `rust/Cargo.toml`, immediately after
   `keywords`.
2. Add `categories.workspace = true` directly below `keywords.workspace = true` in the 12
   manifests that take the shared default: `analytics-web-srv`, `flight-sql-srv`, `http-gateway`,
   `monolith`, `object-cache-srv`, `perfetto`, `redis-exporter`, `telemetry`,
   `telemetry-ingestion-srv`, `telemetry-maintenance-srv`, `telemetry-sink`, `tracing`.
3. Add a literal `categories = [...]` line (per the override table) below `keywords.workspace = true`
   in the 11 overriding manifests: `public`, `analytics`, `ingestion`, `otel-ingestion`,
   `datafusion-extensions`, `transit`, `transit/derive`, `tracing/proc-macros`,
   `micromegas-proc-macros`, `auth`, `object-cache`.
4. Add the literal `categories` line to `rust/datafusion-wasm/Cargo.toml`, below its literal
   `keywords`.
5. `CHANGELOG.md`: a bullet under `## Unreleased` recording the new shared `categories` metadata
   and the per-crate overrides.
6. Run the verification below.

## Files to Modify

- `rust/Cargo.toml`
- `rust/{analytics,analytics-web-srv,auth,datafusion-extensions,datafusion-wasm,flight-sql-srv,http-gateway,ingestion,micromegas-proc-macros,monolith,object-cache,object-cache-srv,otel-ingestion,perfetto,public,redis-exporter,telemetry,telemetry-ingestion-srv,telemetry-maintenance-srv,telemetry-sink,tracing,transit}/Cargo.toml`
- `rust/tracing/proc-macros/Cargo.toml`
- `rust/transit/derive/Cargo.toml`
- `CHANGELOG.md`

## Trade-offs

- **Uniform opt-in vs. published-only opt-in.** Opting in only the published crates would touch
  eight fewer files, but it would make `categories` the one shared field with a per-crate exception
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
   two or its override row from the table above. A crate showing `[]` means its
   `categories.workspace = true` line is missing.

2. From `rust/datafusion-wasm/`, run the same command — it is a separate workspace, so step 1 does
   not cover it. Expect
   `["wasm", "database-implementations", "development-tools::profiling", "development-tools::debugging"]`.

3. From the repo root. Not automated because Cargo carries no category-slug list of its own — only crates.io validates
   slugs, and only at publish time. Diff the slugs that actually landed in both workspaces'
   manifests against the registry's live slug set (the API requires a descriptive `User-Agent`):

   ```python3
   import json, subprocess, urllib.request as u
   def get(url):
       req = u.Request(url, headers={"User-Agent": "micromegas-dev (madesroches@gmail.com)"})
       return json.load(u.urlopen(req))
   reg = {c["id"] for c in get("https://crates.io/api/v1/categories?per_page=100")["categories"]}
   reg |= {c["id"] for c in get("https://crates.io/api/v1/categories/development-tools")["category"]["subcategories"]}
   used = set()
   for ws in ("rust", "rust/datafusion-wasm"):
       out = subprocess.run(["cargo", "metadata", "--no-deps", "--format-version", "1"],
                             cwd=ws, capture_output=True, text=True, check=True).stdout
       used.update(c for p in json.loads(out)["packages"] for c in p["categories"])
   print("unknown slugs:", used - reg or "none")
   ```

   Expect `unknown slugs: none`.

## Decisions

- `database-implementations` is not part of the shared default: it goes only on the crates that
  implement the lakehouse (`micromegas`, `micromegas-analytics`, `micromegas-ingestion`,
  `micromegas-otel-ingestion`, `micromegas-datafusion-extensions`, and the standalone
  `micromegas-datafusion-wasm`). Pure instrumentation, serialization, and trace-export crates —
  `micromegas-tracing`, `micromegas-transit`, `micromegas-telemetry`, `micromegas-telemetry-sink`,
  `micromegas-perfetto` — must not carry it, and neither does `micromegas-object-cache`.

## Open Questions

None.
