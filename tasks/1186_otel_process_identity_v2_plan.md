# OTEL: Binary-Constant Resource Attributes in Process Identity Plan

## Overview

Extend the OTLP process identity formula to include all binary-constant resource attributes
(`os.*`, `host.arch`, `service.version`, `telemetry.sdk.*`, `process.runtime.*`) under the
existing `NS_OTEL_PROCESS_V1` namespace — no namespace bump. The motivating case is Windows +
WSL sibling processes that share every current V1 field but differ in `os.type`, causing them
to collapse onto the same `process_id`.

## Current State

### Identity formula (V1)

`rust/otel-ingestion/src/identity.rs:170` — `process_id_from_resource` hashes eight fields
under `NS_OTEL_PROCESS_V1`:

```
host.id · host.name · process.pid · process.creation.time ·
service.namespace · service.name · service.instance.id · process.owner
```

All fields are passed through `attr_norm` (lower-case + trim) except `process.pid` and
`process.creation.time` which are `attr_raw`. `process.owner` was added after initial shipment
without a V2 bump, as documented in the comment at `identity.rs:169`.

The comment at `identity.rs:5` makes the invariant explicit: "Once the formula ships it cannot
change without a `_V2` namespace UUID".

### Why V1 can't cover the Windows/WSL case

A native Windows process and its WSL/Linux counterpart on the same machine can share all eight
V1 fields when neither emits `process.pid` nor `process.creation.time` (e.g. Claude Code). The
only distinguishing information is `os.type` ("windows" vs "linux"), `os.version`, and
`host.arch` — none of which are in V1.

### Tests

`rust/otel-ingestion/tests/identity_tests.rs` — comprehensive V1 regression suite using
the `resource_with(&[("k","v"), ...])` helper. Pattern to follow for V2 tests.

### Docs

`mkdocs/docs/otlp/index.md:68` — quotes the V1 formula literally. Needs updating to V2.

## Design

### Principle: extend the formula in-place, no namespace bump

Adding new fields under the same `NS_OTEL_PROCESS_V1` namespace is the same approach used when
`process.owner` was added after initial shipment (see `identity.rs:169`). The prerequisite is
that the OTLP ingestion feature is new enough that re-deriving existing `process_id`s is
acceptable — any in-flight processes get a new `process_id` on their next batch, which is a
one-time break rather than an ongoing inconsistency.

No new constant is needed. The V1 namespace UUID is already load-bearing and stays as-is.

### Extended formula

All fields pass through `attr_norm` (lower-case + trim). Fields are appended after the current
eight (host.id, host.name, process.pid, process.creation.time, service.namespace, service.name,
service.instance.id, process.owner):

```
process_id = uuid_v5(NS_OTEL_PROCESS_V1,
    host.id · host.name ·
    process.pid · process.creation.time ·
    service.namespace · service.name · service.instance.id ·
    process.owner ·
    os.type · os.version · os.name · os.description · os.build_id ·
    host.arch · host.type ·
    host.image.id · host.image.name · host.image.version ·
    host.cpu.model.id · host.cpu.model.name · host.cpu.family ·
    host.cpu.vendor.id · host.cpu.stepping · host.cpu.cache.l2.size ·
    service.version ·
    telemetry.sdk.name · telemetry.sdk.language · telemetry.sdk.version ·
    process.runtime.name · process.runtime.version · process.runtime.description)
```

`·` denotes `\x1F` (ASCII unit separator) to prevent boundary collisions.

`process.pid` and `process.creation.time` remain `attr_raw` (no case folding) — they are
numeric/timestamp values rather than free-form strings.

**Excluded from identity** (per issue): `process.command`, `process.command_args`,
`process.command_line`, `process.parent_pid`, `process.interactive`, `process.linux.cgroup` —
these are invocation-specific and can change across retries or on the same logical process.

### Impact on existing data

Adding fields to the key string changes the UUIDv5 hash for every resource, including those
where all new fields are empty. In-flight processes get a new `process_id` on their next batch.
Existing rows in `processes`/`streams`/`blocks` are unaffected; orphaned old rows decay under
the normal retention policy.

## Implementation Steps

1. **Extend `process_id_from_resource`** in `rust/otel-ingestion/src/identity.rs` to append all
   new fields after `process.owner` in the key string (see formula in Design). Keep the same
   `NS_OTEL_PROCESS_V1` namespace and `attr_norm` for all new fields.

2. **Update the doc-comment** on `process_id_from_resource` to list the full extended field set
   and note that fields were added in-place (same pattern as the `process.owner` precedent).
   Remove the "Any further change must bump the namespace" sentence from `identity.rs:169` and
   replace it with a description of the in-place extension policy and its conditions: in-place
   extension is acceptable only while the feature is pre-GA and re-deriving existing
   `process_id`s is acceptable; once the feature is GA a formula change requires a new namespace
   UUID.

3. **Add unit tests** in `rust/otel-ingestion/tests/identity_tests.rs`:
   - `windows_and_wsl_differ` — two resources with identical host/pid/service/owner but
     `os.type = "windows"` vs `"linux"` produce different `process_id`s.
   - `process_id_is_stable_with_new_fields` — a fixed resource with known values for the new
     fields produces a hardcoded expected UUID (regression lock). Choose a resource that
     exercises at least one field from each namespace group.

4. **Update docs** in `mkdocs/docs/otlp/index.md`:
   - Replace the formula block at line 68 with the extended formula (all fields, grouped by
     namespace for readability).
   - Add a short note that the formula was extended in-place under the same namespace UUID.

5. **Run CI**: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test` from
   `rust/`, then `python3 build/rust_ci.py`.

## Files to Modify

- `rust/otel-ingestion/src/identity.rs` — extended formula, updated doc-comment
- `rust/otel-ingestion/tests/identity_tests.rs` — two new tests
- `mkdocs/docs/otlp/index.md` — formula block update, in-place extension note

## Trade-offs

- **Why not a narrower fix — just `os.type` + `host.arch`?**  The issue enumerates the full
  set of binary-constant attributes per OTel semantic conventions. Including them all means no
  further formula change is needed for the "more constant attributes" class of problem. The
  marginal cost of extra empty fields in the hash string is negligible.

- **Why keep `process.pid` and `process.creation.time` as `attr_raw`?**  These are
  numeric/timestamp values — case folding adds no value and could obscure unexpected non-string
  representations (e.g. an SDK emitting an integer PID). The `attr_norm` convention is for
  free-form text where upper/lower differences are cosmetic.

## Documentation

- `mkdocs/docs/otlp/index.md` — formula update + migration note (see Implementation Step 5).
- No changelog entry required (no user-facing API change, formula upgrade is transparent at the
  SDK level).

## Testing Strategy

- **Regression lock** (`process_id_is_stable_with_new_fields`): hardcode the expected UUID for
  a canonical resource with non-empty new fields. Fails immediately if the formula accidentally
  changes in the future.
- **Semantic test** (`windows_and_wsl_differ`): directly exercises the motivating scenario from
  the issue.
- **Existing tests** in `identity_tests.rs` verify relative behavior only (equality, inequality,
  normalization); the regression-lock test in step 3 is the actual guard against accidental
  formula changes.

## Open Questions

1. **Field order within the extended string** — the plan groups by OTel namespace
   (`os.*`, `host.*`, `service.*`, `telemetry.sdk.*`, `process.runtime.*`). As long as the
   order is documented in the source and locked by the regression test, any stable order is
   correct.
