# Latest OTLP Conformance Plan (opentelemetry-proto 0.32)

## Overview

The `opentelemetry-proto` 0.32 upgrade (branch `fix/dependabot-336-opentelemetry-sdk`)
pulls in proto definitions v1.10.0, which add the **profiling-signal string-interning
fields** to the shared common types: `AnyValue::StringValueStrindex(i32)` and
`KeyValue::key_strindex: i32`. These are references into a `ProfilesDictionary.string_table`
and are used **exclusively by the Profiles signal**, which Micromegas does not ingest.

The branch handles the new `AnyValue::StringValueStrindex` arm by emitting the raw integer
index stringified (`idx.to_string()`). That is incorrect: index `5` means "entry #5 in a
dictionary that does not exist for logs/metrics/traces," not the string `"5"`. It fabricates
plausible-looking garbage and — in the identity path — would fold a meaningless integer into
the load-bearing `process_id`/`stream_id` UUIDv5 hash.

This plan makes the 0.32 upgrade **spec-conformant**: handle the interning fields the way the
OTLP spec mandates for non-Profiling receivers (treat as absent/empty, log a warning), audit
the rest of the 0.32 delta, and lock the behavior in with tests. **Profiles ingestion is
explicitly out of scope** (see "Profiles: deferred").

## Goal (one sentence)

Make Micromegas's OTLP decode path conform to the opentelemetry-proto 0.32 spec by treating
the profiling-only string-interning fields as absent (never as data), instead of stringifying
their dictionary indices.

## Current State

The OTLP ingestion feature (logs/metrics/traces) is implemented and complete — see
`tasks/completed/otlp_ingestion_plan.md`. The workspace enables
`opentelemetry-proto` features `["gen-tonic-messages", "logs", "metrics", "trace", "with-serde"]`
— **not** `profiles`, so the `Profile`/`Sample`/`ProfilesDictionary` types are not even
compiled in. Only the interning fields leaked into the shared `common.v1` types are present.

The proto's own documentation is explicit about the contract
(`opentelemetry.proto.common.v1`, both `AnyValue::StringValueStrindex` and
`KeyValue::key_strindex`):

> Note: This is currently used exclusively in the Profiling signal. Implementers of OTLP
> receivers for signals other than Profiling should treat the presence of this value as a
> non-fatal issue. Log an error or warning indicating an unexpected field intended for the
> Profiling signal and process the data as if this value were absent or empty, ignoring its
> semantic content for the non-Profiling signal.

### `AnyValue::StringValueStrindex` — three sites, all wrong (stringify the index)

- `rust/analytics/src/lakehouse/otel/attrs.rs:49` — `any_value_to_jsonb`
  ```rust
  Some(Av::StringValueStrindex(idx)) => JsonbValue::String(Cow::Owned(idx.to_string())),
  ```
- `rust/analytics/src/lakehouse/otel/attrs.rs:109` — `any_value_to_string`
  ```rust
  Some(Av::StringValueStrindex(idx)) => idx.to_string(),
  ```
- `rust/otel-ingestion/src/identity.rs:67` — `attr_to_string` (feeds the UUIDv5 identity hash)
  ```rust
  Some(any_value::Value::StringValueStrindex(idx)) => idx.to_string(),
  ```

The `None` arm directly below each of these already produces the correct absent value
(`JsonbValue::Null` / `String::new()`). The fix is to make the strindex arm behave identically,
plus a warning.

Recursion is already covered: `any_value_to_jsonb` handles nested `ArrayValue`/`KvlistValue`
by recursing into itself, and `scope_extras` (`attrs.rs:164`) routes scope attributes through
`any_value_to_jsonb`, so fixing the leaf arm fixes every nested case.

### `KeyValue::key_strindex` — already correct by omission

Key lookups never read `key_strindex`:
- `attrs_to_jsonb` (`attrs.rs:71`) and the kvlist walk (`attrs.rs:44`) use `kv.key.clone()`.
- `identity::attr` (`identity.rs:44-48`) matches on `kv.key == key`.

An interned key (where `key` is empty and `key_strindex` is set) therefore becomes an
empty-key entry — i.e., treated as absent, which is exactly the spec behavior. No code change
needed; this just needs a comment and a regression test so it can't silently regress.

### 0.32 delta audit (decoder-relevant)

Per the `opentelemetry-proto` CHANGELOG, 0.32 = proto v1.10.0 + a `schemars` bump + a bug fix
in *their* SDK→proto `transform` module (`InstrumentationScope` version/attrs preserved when a
log target is set). Micromegas **decodes** proto bytes and does not use the `transform` module,
so that fix is irrelevant to us. The only decoder-relevant change is the interning fields.

Separately, the branch flipped `json_tests.rs::bare_number_timestamp_rejected` →
`bare_number_timestamp_accepted`: 0.32's serde deserializer now accepts a bare-number
`timeUnixNano` (lenient proto3 JSON). The new test asserts the parsed value, not just `is_ok()`.
This is a legitimate, already-handled behavior change — keep it.

## Design

### Core fix: interning fields decode to "absent," with a warning

Make all three `StringValueStrindex` arms mirror their adjacent `None` arm:

| Site | Function | New behavior |
|---|---|---|
| `attrs.rs:49` | `any_value_to_jsonb` | `JsonbValue::Null` |
| `attrs.rs:109` | `any_value_to_string` | `String::new()` |
| `identity.rs:67` | `attr_to_string` | `String::new()` |

This is the spec's "process the data as if this value were absent or empty."

### Warning policy (satisfy the spec's "log a warning" without log spam)

The converters are pure leaf functions called in tight per-attribute, per-row loops. A `warn!`
inside the leaf would spam if a misbehaving producer floods strindex values. The spec wants a
warning, not a per-value warning. Recommended approach:

- **Leaf converters stay silent and pure** — they just return the absent value.
- **Each entry point that walks attributes counts strindex occurrences and emits a single
  throttled warning per block / per resource.** Concretely:
  - In `otel_logs_block_processor` / `otel_metrics_block_processor` / `otel_spans_block_processor`:
    while walking records, if any `AnyValue` carries `StringValueStrindex` (or any `KeyValue`
    carries a nonzero `key_strindex`), `warn!` once per block with a count and the
    `process_id`/scope for triage. Use the existing `micromegas_tracing::prelude::*` (already
    imported in these processors — e.g. `logs_block_processor.rs:24`).
  - In `identity.rs`, the resource walk that builds `process_id` is the natural single-warn
    site (one resource = one potential warning). `otel-ingestion` already depends on
    `micromegas-tracing`.

This keeps the should-never-happen path observable (so we notice if a profiling producer ever
points at a non-profiles endpoint) without flooding logs.

**Minimal-acceptable fallback** (if the per-block counter is judged over-engineered for an
unreachable path): drop the leaf arms to the absent value with no logging, or a `debug!` in the
leaf. The correctness fix (no stringified index) is the load-bearing part; the warning is a
spec SHOULD. Recommend the per-block warning, but the correctness fix must land regardless.

### Why a shared helper is not worth it

The three arms live in two crates (`analytics` and `otel-ingestion`) with different value types
(`JsonbValue` vs `String`) and no shared module. A one-line arm in each, matching the existing
`None` arm beside it, is clearer than introducing a cross-crate helper for three lines. Keep it
local (DRY does not mean deduplicating a one-liner across incompatible return types).

## Implementation Steps

1. **`rust/analytics/src/lakehouse/otel/attrs.rs`** — change both `StringValueStrindex` arms
   (lines 49, 109) to return the absent value (`JsonbValue::Null` / `String::new()`). Replace
   the misleading "emit the index as a string" comments with a note citing the spec
   (profiling-only; treat as absent for logs/metrics/traces).
2. **`rust/otel-ingestion/src/identity.rs`** — change the `StringValueStrindex` arm (line 67) to
   `String::new()` with the same corrected comment. This is the highest-stakes site (identity
   hash); call that out in the comment so nobody "optimizes" it back to stringifying.
3. **Warning (recommended):** add a single throttled `warn!` per block in the three OTel block
   processors and per resource in `identity.rs`, gated on detecting any strindex /
   nonzero-`key_strindex`. Use `micromegas_tracing::prelude::*`.
4. **`KeyValue::key_strindex` comment** — add a short comment at the key-reading sites
   (`attrs.rs` kvlist walk + `attrs_to_jsonb`; `identity.rs::attr`) noting that `key_strindex`
   is intentionally ignored (profiling-only; absent ⇒ empty key per spec).
5. **Tests** — see Testing Strategy.
6. **Keep** the `bare_number_timestamp_accepted` test change from the branch as-is.
7. Run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test`, and
   `python3 build/rust_ci.py`.

## Files to Modify

- `rust/analytics/src/lakehouse/otel/attrs.rs` (two arms + comments; key_strindex comment)
- `rust/otel-ingestion/src/identity.rs` (one arm + comments; key_strindex comment)
- `rust/analytics/src/lakehouse/otel/logs_block_processor.rs`,
  `metrics_block_processor.rs`, `spans_block_processor.rs` (optional per-block warning)
- New/extended unit tests (see Testing Strategy) — under the crate `tests/` folders per project
  convention.

No `Cargo.toml`, schema, or wire-format changes. The dependency bump itself already landed on
the branch.

## Profiles: deferred (and why)

We are **not** adding the OTLP Profiles signal now. Recorded so the analysis isn't lost:

- **Unstable wire format.** Profiles is `v1development` (package
  `opentelemetry.proto.profiles.v1development`, HTTP path `/v1development/profiles`, fields
  marked `Status: [Development]`). It can take breaking changes in any proto release. That
  conflicts with Micromegas's "identity formulas and persisted schemas are load-bearing and
  unchangeable" contract (`_V1`→`_V2` rule).
- **Not compiled in.** The `profiles` cargo feature is off; adding the signal means enabling it
  and pulling in the whole `Profile`/`Sample`/`ProfilesDictionary` model.
- **Architecture mismatch.** The data model is dictionary-encoded to the bone: one request
  carries a single shared `ProfilesDictionary` (string/function/location/mapping/stack/link/
  attribute tables) and everything else is integer indices into it. The existing OTLP
  architecture stores one self-contained, independently-decodable block per `Resource` — but the
  profiles dictionary is request-level and shared across resources, so per-resource blocks can't
  be decoded in isolation without duplicating/subsetting the dictionary.
- **No natural tabular target.** A profile is a tree of stacks, not flat rows like
  `log_entries`/`measures`/`otel_spans`. Materializing it means either huge fan-out (one row per
  resolved sample/frame) or storing the raw `original_payload` (pprof/JFR) blob and rendering
  client-side — a real design exercise.
- **No demand + high cost.** Claude Code (the driving use case for OTLP) does not emit profiles,
  and continuous profiling is the highest-volume signal.

If/when we revisit: the low-regret shape is "store raw `ProfilesData` blobs, resolve the
dictionary at query time, no stable persisted schema while it's `v1development`." That would be
its own design doc (`otlp_profiles_support_plan.md`).

## Testing Strategy

Add focused unit tests asserting the absent-not-fabricated behavior:

- **`attrs.rs` (`any_value_to_jsonb`)**: an `AnyValue { value: Some(StringValueStrindex(5)) }`
  converts to `JsonbValue::Null` (NOT the string `"5"`). Add a nested case: a `KvlistValue`
  containing a strindex value → the inner key maps to JSON null, proving recursion is covered.
- **`attrs.rs` (`any_value_to_string`)**: `StringValueStrindex(5)` → `""`.
- **`identity.rs` (`attr_to_string`)**: `StringValueStrindex(5)` → `""`. Plus an identity-stability
  test: two resources identical except one carries a strindex attribute where the other omits the
  attribute entirely produce the **same** `process_id` (proves strindex is treated as absent in
  the hash).
- **`key_strindex` regression**: a `KeyValue { key: "", key_strindex: 7, value: Some(string "x") }`
  in an attribute set does **not** produce a `"7"` key (it yields an empty-key entry, i.e. absent).
- **Keep** `json_tests.rs::bare_number_timestamp_accepted`.

Existing test fixtures already set `key_strindex: 0` in the `s_kv`/`i_kv`/`kv` helpers
(`tests/fixtures.rs`, `tests/block_tests.rs`, `tests/identity_tests.rs`) — extend a helper or add
a local builder for the nonzero `key_strindex` case rather than changing the shared default.

Run `cargo test`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`, and
`python3 build/rust_ci.py`. No e2e change needed — this is decode-layer behavior covered by unit
tests.

## Trade-offs

- **Absent vs. preserve-as-debug-string**: chose absent (spec-mandated). Preserving the index in a
  side channel (e.g. `properties.otel._strindex`) was considered and rejected — the index is
  meaningless without the dictionary we don't have, so storing it only invites someone to treat it
  as data later.
- **Per-block warning vs. silent**: recommend a throttled per-block warning (spec SHOULD, and it's
  the only way we'd notice a misconfigured profiling producer hitting a logs/metrics/traces
  endpoint). Accept silent/`debug!` as a fallback; the correctness fix is non-negotiable.
- **Local one-liners vs. shared helper**: kept local — two crates, two return types, three lines.

## Open Questions

1. Per-block warning vs. silent — confirm the recommended throttled warning is wanted, or prefer
   the minimal silent fix. (Either way the index is no longer stringified.)
