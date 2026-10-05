# Crate Metadata for Standalone and Entry-Point Crates Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1649

## Overview

Every published crate inherits the workspace `keywords` (`observability`, `telemetry`,
`analytics`) and `homepage` (`https://micromegas.info/`), and most descriptions read
"…, part of micromegas". Several crates are useful on their own (`micromegas-perfetto` has
~156k downloads, mostly via `rspack_tracing_perfetto`) or are the main entry points into
Micromegas, and that generic metadata makes them hard to find on crates.io and docs.rs. This
plan gives seven crates their own description, keywords, homepage and docs.rs landing content,
and adds two keywords to the `micromegas` umbrella crate. It touches only manifests, READMEs,
crate-level doc attributes and the changelog. No code behavior changes.

## Current State

- `rust/Cargo.toml:7-16`: `[workspace.package]` sets `homepage`, `keywords`, `categories`,
  `documentation = "https://docs.rs/micromegas"`. No crate opts into `documentation.workspace`.
  Only `datafusion-wasm` sets `documentation`, so crates.io falls back to each crate's own docs.rs
  page. That fallback is correct, so `documentation` stays untouched.
- Per-crate manifests (`description`, then `keywords.workspace = true`, `homepage.workspace = true`):

  | Crate | Manifest | Current description | `lib.rs` crate docs | README |
  |---|---|---|---|---|
  | `micromegas-perfetto` | `rust/perfetto/Cargo.toml` | "perfetto trace writer, part of micromegas" | one line `//!` | one sentence |
  | `micromegas-object-cache` | `rust/object-cache/Cargo.toml` | "object range cache for micromegas: …" | none | one paragraph |
  | `micromegas-datafusion-extensions` | `rust/datafusion-extensions/Cargo.toml` | "WASM-compatible DataFusion UDF extensions for micromegas" | none | good function list |
  | `micromegas-transit` | `rust/transit/Cargo.toml` | "low overhead serialization, part of micromegas" | two-line `//!` | one sentence |
  | `micromegas-tracing` | `rust/tracing/Cargo.toml` | "instrumentation module, part of micromegas" | good `//!` | short |
  | `micromegas-otel-ingestion` | `rust/otel-ingestion/Cargo.toml` | "OTLP/HTTP ingestion adapter for micromegas" | good `//!` | short |
  | `micromegas-auth` | `rust/auth/Cargo.toml` | "Authentication providers for Micromegas (API keys, OIDC)" | good `//!` | short |

- None of the crates sets `readme`, but Cargo auto-detects `README.md` in the package root, so
  crates.io already shows each README.
- No crate uses `#![doc = include_str!("../README.md")]` yet. CI runs `cargo test --doc`
  (`build/rust_ci.py:16`), so any Rust code block in an included README becomes a compiled and
  executed doctest.
- Docs site: `mkdocs/mkdocs.yml:3` sets `site_url: https://micromegas.info/docs/`, so
  `mkdocs/docs/<path>.md` is served at `https://micromegas.info/docs/<path>/`. Two of the
  homepages suggested in the issue are wrong:
  - `/docs/native/` is the **C ABI** (`micromegas-capi`) page, not Rust instrumentation. No Rust
    instrumentation page exists (`mkdocs/docs/rust/` is empty).
  - `/docs/admin/functions-reference/` documents the admin UDTFs (`list_view_sets`, …). The
    JSONB/histogram UDFs that `datafusion-extensions` provides are documented in
    `query-guide/functions-reference.md`.
  - The other suggested pages exist: `admin/object-cache.md`, `architecture/caching.md`,
    `otlp/index.md`, `admin/authentication.md`.

## Design

### Per-crate metadata

All keywords satisfy crates.io rules: at most 5, at most 20 chars, ASCII alphanumeric plus
`-`/`_`, starting with a letter.

| Crate | `description` | `keywords` | `homepage` |
|---|---|---|---|
| `micromegas-perfetto` | "Streaming writer for Perfetto trace files: encodes thread and async spans as protobuf TracePackets, with bundled Perfetto proto bindings. From the Micromegas project." | `perfetto`, `trace`, `profiling`, `protobuf`, `chrome-tracing` | `https://docs.rs/micromegas-perfetto` |
| `micromegas-object-cache` | "Range-aware read cache for `object_store`: a byte-range cache engine plus an ObjectStore client that routes reads through a shared cache service and falls back to the origin. From the Micromegas project." | `object-store`, `cache`, `s3`, `parquet`, `range-requests` | `https://micromegas.info/docs/architecture/caching/` |
| `micromegas-datafusion-extensions` | "Apache DataFusion UDFs that also compile to wasm32: JSONB parsing and querying, histograms, colors, binning and math. From the Micromegas project." | `datafusion`, `udf`, `jsonb`, `wasm`, `sql` | `https://micromegas.info/docs/query-guide/functions-reference/` |
| `micromegas-transit` | "Low-overhead binary serialization for plain-old-data structs, with reflection metadata for decoding heterogeneous event queues. From the Micromegas project." | `serialization`, `binary`, `pod`, `encoding`, `reflection` | `https://docs.rs/micromegas-transit` |
| `micromegas-tracing` | "Low-overhead logs, metrics and spans for Rust applications and game engines. The instrumentation library of Micromegas." | `profiling`, `instrumentation`, `logging`, `metrics`, `gamedev` | `https://docs.rs/micromegas-tracing` |
| `micromegas-otel-ingestion` | "OTLP/HTTP ingestion of OpenTelemetry logs, metrics and traces into the Micromegas data lake." | `opentelemetry`, `otlp`, `ingestion`, `observability` | `https://micromegas.info/docs/otlp/` |
| `micromegas-auth` | "API key and OIDC authentication providers for axum and tonic services, used by Micromegas." | `oidc`, `api-key`, `authentication` | `https://micromegas.info/docs/admin/authentication/` |
| `micromegas` | unchanged | `observability`, `telemetry`, `analytics`, `opentelemetry`, `datafusion` | unchanged (workspace) |

Each override replaces the `keywords.workspace = true` / `homepage.workspace = true` line in
place, keeping the manifest's field order. `categories` stay as they are.

The wording of these descriptions is a starting point. The implementer may tighten it, but each
must name what the crate does first and mention Micromegas last, if at all.

### docs.rs landing pages

Two different treatments, depending on whether `lib.rs` already has substantial `//!` docs:

1. **No meaningful crate docs**: `perfetto` (one-line `//!`), `object-cache` (none),
   `datafusion-extensions` (none), `transit` (two-line `//!`). Replace any existing `//!` lines
   with `#![doc = include_str!("../README.md")]` at the top of `lib.rs`, before the existing
   `#![allow(...)]` attributes. Then the README is the single source for both crates.io and
   docs.rs.
2. **Good `//!` docs**: `tracing`, `otel-ingestion`, `auth`. Leave `lib.rs` alone and expand the
   README only (see below). Including the README here would duplicate or displace good
   hand-written crate docs.

### README content

READMEs that get included (treatment 1) become rustdoc, so they must follow these rules:
- Use absolute URLs only. Relative links break on docs.rs.
- Tag every non-Rust fenced block (`toml`, `sql`, `text`, `bash`). An untagged block is compiled
  as a Rust doctest.
- Rust examples must compile and pass under `cargo test --doc`.

Per crate:
- **perfetto**: what it is (a streaming Perfetto `TracePacket` writer over any `AsyncWriter`
  sink), when to use it (generate traces viewable in ui.perfetto.dev from your own span data), a
  usage example, and links (docs.rs, repo, Micromegas home). The example defines a local newtype
  sink wrapping `Arc<Mutex<Vec<u8>>>` with `#[async_trait::async_trait] impl AsyncWriter` (like
  `SharedBufferAsyncWriter` in `rust/perfetto/tests/async_streaming_writer_tests.rs`; the orphan
  rule forbids implementing it for `Vec<u8>` in a doctest) and runs `PerfettoWriter::new` → `emit_process_descriptor` → `emit_thread_descriptor` →
  `emit_span` → `flush`, driven by `#[tokio::main]`. tokio is a regular dependency, and the
  workspace enables its `macros` and `rt-multi-thread` features. Mention the `protos` module for
  users who build packets by hand.
- **object-cache**: what the two halves are (the range cache engine with its memory/foyer
  backends, and the `CacheClientStore` client with circuit breaker and origin fallback), when to
  use it (several processes reading overlapping ranges of the same immutable objects), the
  write-once/no-invalidation assumption, and the `foyer` feature. Include an example that
  constructs a `CacheClientStore` with `CacheClientStore::new(cache_base_url, api_key, direct)`
  over an in-memory origin (`object_store::memory::InMemory`) without issuing reads. Links:
  `architecture/caching/`, `admin/object-cache/`, docs.rs, repo.
- **datafusion-extensions**: extend the existing function list to cover everything
  `register_extension_udfs` registers (add Color, Math, Binning and Properties groups plus the
  missing JSONB entries). Add a short "Usage" section that calls
  `micromegas_datafusion_extensions::register_extension_udfs(&ctx)` on a `SessionContext`. Replace the home-page link with
  `https://micromegas.info/docs/query-guide/functions-reference/` and keep the repo link. The
  issue says the README is already good, so keep this edit small.
- **transit**: what it is (memcpy-style serialization of `#[repr(C)]` POD values plus
  `Reflect`/`UserDefinedType` metadata, so a reader can parse a heterogeneous queue without the
  writer's types), when to use it (high-throughput in-process event buffers, not a general
  schema-evolution format), and a minimal round-trip example with `write_any` and
  `read_consume_pod`. Links: docs.rs, repo.
- **tracing / otel-ingestion / auth** (README only, crates.io-facing): one or two sentences on
  what it does, a short usage snippet or pointer (for `tracing`, a `Cargo.toml` dependency line
  plus `#[span_fn]`, `span_scope!` and `info!` already documented in `lib.rs`), and the specific docs
  link (`otlp/`, `admin/authentication/`). For `tracing`, link to docs.rs as the primary
  reference plus the Unreal and getting-started pages. These READMEs are not included in
  rustdoc, but Rust code blocks should still be tagged `rust` and kept correct.

### Out of scope

- A dedicated Perfetto or Rust-instrumentation page on micromegas.info. When one exists, the
  homepages of `perfetto` and `tracing` can switch from docs.rs to it.
- Internal crates (`analytics`, `ingestion`, `telemetry`, `telemetry-sink`, `proc-macros`,
  `object-cache-srv`, `capi`, …) keep the shared metadata.
- The `http-gateway` crates.io name collision, which matters only if that crate is ever
  published.

## Implementation Steps

1. Update `[package]` in the seven manifests listed in Design: replace `description`, and change
   `keywords.workspace = true` and `homepage.workspace = true` to explicit values. In
   `rust/public/Cargo.toml`, replace `keywords.workspace = true` with the five-keyword list.
2. Rewrite `rust/perfetto/README.md`, `rust/object-cache/README.md` and
   `rust/transit/README.md`, and edit `rust/datafusion-extensions/README.md`, following the README
   rules above.
3. Add `#![doc = include_str!("../README.md")]` to the top of `rust/perfetto/src/lib.rs`,
   `rust/object-cache/src/lib.rs`, `rust/datafusion-extensions/src/lib.rs` and
   `rust/transit/src/lib.rs`, removing the existing `//!` lines in perfetto and transit.
4. Expand `rust/tracing/README.md`, `rust/otel-ingestion/README.md` and `rust/auth/README.md`.
5. Run the checks in Testing Strategy.
6. Add a `CHANGELOG.md` Unreleased entry (crates.io/docs.rs metadata for the listed crates).

## Files to Modify

- `rust/perfetto/Cargo.toml`, `rust/perfetto/README.md`, `rust/perfetto/src/lib.rs`
- `rust/object-cache/Cargo.toml`, `rust/object-cache/README.md`, `rust/object-cache/src/lib.rs`
- `rust/datafusion-extensions/Cargo.toml`, `rust/datafusion-extensions/README.md`,
  `rust/datafusion-extensions/src/lib.rs`
- `rust/transit/Cargo.toml`, `rust/transit/README.md`, `rust/transit/src/lib.rs`
- `rust/tracing/Cargo.toml`, `rust/tracing/README.md`
- `rust/otel-ingestion/Cargo.toml`, `rust/otel-ingestion/README.md`
- `rust/auth/Cargo.toml`, `rust/auth/README.md`
- `rust/public/Cargo.toml`
- `CHANGELOG.md`

## Trade-offs

- **docs.rs as `homepage` where no specific page exists** (perfetto, transit, tracing), instead
  of the generic micromegas.info home. crates.io already shows a docs.rs link through its
  `documentation` fallback, so this duplicates that link. But it keeps visitors of a standalone
  crate from landing on an unrelated product homepage, and it is what the issue asks for.
- **`tracing` homepage diverges from the issue** (`/docs/native/`); see Current State.
- **`datafusion-extensions` homepage diverges from the issue**
  (`/docs/admin/functions-reference/`); see Current State.
- **`object-cache` homepage: architecture page over admin page.** `architecture/caching/`
  explains the library-level design (tiers, no invalidation) that matters to someone embedding
  the crate. The admin page is about deploying the `-srv` binary. The README links both.
- **README include only where crate docs are thin.** One source of truth where it helps, and good
  hand-written `//!` docs stay where they already exist.
- **Doctests over `ignore`.** Compiled examples can't silently rot. The cost is that examples
  must be written against the real API.

## Documentation

No mkdocs pages change. The crate READMEs and the docs.rs landing pages generated from them are
the documentation deliverable. Add a `CHANGELOG.md` entry.

## Testing Strategy

There is no runtime behavior to unit-test. The automated checks are:
- `cargo test --doc -p micromegas-perfetto -p micromegas-object-cache -p micromegas-datafusion-extensions -p micromegas-transit`
  (from `rust/`). This compiles and runs the README examples now included in rustdoc, and it
  catches untagged non-Rust blocks.
- `cargo doc --no-deps` for the same crates, with `RUSTDOCFLAGS="-D warnings"`, to catch broken
  intra-doc links.
- `python3 ../build/rust_ci.py` (fmt, clippy, tests, doc tests) as the normal CI gate.

## Manual Verification

These checks look at crates.io packaging and rendered output, which no unit test reaches:
1. `cargo metadata --no-deps --format-version 1 | jq '.packages[] | select(.name|test("^micromegas")) | {name, description, keywords, homepage}'`
   from `rust/`. Expect the values from the Design table, and every keyword list at 5 entries or
   fewer.
2. `cargo package --list -p micromegas-perfetto --allow-dirty | grep README.md` (and the same
   for the other included crates). Expect `README.md` in each package, because the
   `include_str!` must resolve when docs.rs builds from the published tarball.
3. `cargo doc --no-deps -p micromegas-perfetto --open` (and the same for object-cache, transit,
   datafusion-extensions). The crate landing page should render the README.

## Open Questions

None.
