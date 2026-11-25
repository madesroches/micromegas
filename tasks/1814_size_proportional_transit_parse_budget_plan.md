# Size-Proportional Transit Parse Budget Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1814

## Overview
Stream metadata is client-supplied. It can declare UDTs whose members share an offset, so a
tiny object payload fans out into a huge tree of parsed values (a transit "billion laughs").
#1813 already caps each top-level instance at 4096 visited members. That stops the issue's
`16^6` repro, but the cap is a constant, unrelated to how many bytes the object occupies. A
2-byte object (1 type-index byte + 1 payload byte) can still produce 4096 values, about
160 KB of arena. Every object in a block is a disjoint slice of the block buffer and pays the
same cost again, and the arena is per block. So amplification is still about 80,000x per
block. This plan makes the per-instance budget proportional to the object's size, so total
parse work and arena use are linear in the block's byte count.

## Current State
- `rust/transit/src/parser.rs`: `MAX_UDT_NESTING_DEPTH = 32` and
  `MAX_UDT_NODES_PER_INSTANCE = 4096`. `parse_pod_instance` builds a
  `ParseBudget { depth: 0, nodes_left: MAX_UDT_NODES_PER_INSTANCE }` and delegates to
  `parse_pod_instance_bounded`. That function decrements `nodes_left` once per visited member
  (intrinsic, reference, or nested) and fails with a `corrupt metadata` error at zero.
- Callers of `parse_pod_instance` pass a window sized to the UDT being parsed:
  - `read_dependencies` passes `&buffer[offset..object_end]` in its POD branch only, with
    `object_end` set to `offset + udt.size`. Its `StaticStringDependency` and custom-reader
    branches pass `advance_window(buffer, offset)`, the whole remaining buffer, and then jump
    `offset` to `object_end`. `parse_property_set` (registered for the size-0
    `PropertySetDependency`) bounds `nb_properties` against that unbounded window, so a
    property set with a small declared size can parse properties spanning the rest of the
    buffer, and later objects re-parse them: O(B^2).
  - `parse_object_buffer` passes `&buffer[offset..object_end]` for static-size objects.
  - The custom readers in `rust/tracing/src/parsing.rs` pass `StaticStringRef`/`StringId`
    windows of `metadata.size` and `Property` windows of `property_layout.size`.

  So for the POD and `parse_object_buffer` paths the window length is the object's byte
  footprint.
- `rust/analytics/src/payload.rs` `parse_block` uses one `Bump` for the whole block. Nothing
  is freed between objects.
- Real layouts are sparse. Each intrinsic leaf occupies at least 1 byte, a reference at least
  8, and members don't overlap. Events in `rust/tracing` and the Unreal interop types come to
  roughly 0.1–0.2 visited members per byte. A struct made entirely of `u8` fields reaches 1.
- Tests: `rust/transit/tests/test_corrupt_input.rs` already has the `pod_udt`/`member`
  helpers and the #1813 recursion and fan-out cases.

## Design
Derive the initial budget from the window length, keeping the existing ceiling:

```rust
// Legitimate layouts visit at most ~1 member per byte (non-overlapping leaves of >= 1 byte);
// the slack covers a few wrapper structs around tiny objects.
const UDT_NODES_PER_BYTE: usize = 2;
const UDT_NODES_SLACK: usize = 8;

fn instance_node_budget(window_len: usize) -> usize {
    window_len
        .saturating_mul(UDT_NODES_PER_BYTE)
        .saturating_add(UDT_NODES_SLACK)
        .min(MAX_UDT_NODES_PER_INSTANCE)
}
```

`parse_pod_instance` seeds `ParseBudget.nodes_left` with `instance_node_budget(object_window.len())`.
Nothing else changes: same signature, same counting, same error path. The exhaustion message
reports the computed budget, not the constant (e.g. `corrupt metadata: member budget of {n}
for {len}-byte instance exhausted at udt {name}`).

**Bound.** Objects in a block are disjoint, and each one costs at least 2 buffer bytes (type
index plus a nonzero static size; size 0 means dynamic, which takes the custom-reader path). So
the visited members across a block are at most `2·B + 8·(B/2) = 6·B` for a buffer of `B` bytes.
Parse work and arena growth become linear in the decompressed block size. `read_dependencies` has the same
bound once its non-POD branches get bounded windows (step 2). Bounding the block's own LZ4 decompression (`decompress` in
`rust/telemetry/src/compression.rs` has no output cap) is a separate amplification vector and
out of scope here.

**Validation at ingestion** (the issue's second suggestion) is not added. The parser is the one
place every metadata source goes through, and the runtime budget covers it, which is the same
reasoning #1813 used against load-time cycle rejection.

## Implementation Steps
1. `rust/transit/src/parser.rs`: add `UDT_NODES_PER_BYTE`, `UDT_NODES_SLACK`, and
   `instance_node_budget`. In `parse_pod_instance`, seed `nodes_left` with it. Add
   `initial_nodes` and `instance_len` to `ParseBudget`, set in `parse_pod_instance` and used in
   the exhaustion `bail!` to report `{n}` and `{len}`. In `parse_pod_instance_bounded`, reserve
   `udt.members.len().min(budget.nodes_left)` instead of `udt.members.len()` in
   `with_capacity_in`, so the reservation isn't set by the metadata. Update the comment above
   the constants.
2. `rust/transit/src/parser.rs` `read_dependencies`: pass `&buffer[offset..object_end]` to the
   `StaticStringDependency` branch and to custom readers (as `parse_object_buffer` already
   does), and remove the now-stale comments there. Rust and Unreal writers emit exact sizes
   (`PropertySetDependency::get_value_size`, `StaticStringDependency::get_value_size`, Unreal
   `Serializer<PropertySetDependency>::GetSize`), so bounding is safe for real data.
3. `rust/transit/tests/test_corrupt_input.rs`: add the regression tests below.
4. `CHANGELOG.md`: Unreleased fix entry citing #1814 (done by the `pr` step).

## Files to Modify
- `rust/transit/src/parser.rs`
- `rust/transit/tests/test_corrupt_input.rs`
- `CHANGELOG.md`

## Trade-offs
- **Size-proportional per-instance budget vs. a per-block budget threaded through
  `parse_object_buffer`/`read_dependencies`.** A per-block counter needs a new parameter on
  `parse_pod_instance`, and the custom readers in `rust/tracing` have no budget to pass.
  Tying the budget to the object's own bytes gives the same linear-per-block bound with no
  signature change, and it doesn't let one object starve its siblings.
- **Arena size limit (`Bump::set_allocation_limit`).** When an allocation goes over the limit,
  `alloc`/`Vec::with_capacity_in` abort the process instead of returning `Err`, so a limit
  would turn an OOM into the same abort. A counted budget fails cleanly.
- **Constants vs. configuration.** These are sanity limits on malformed input, not tuning
  knobs, so they stay hard-coded, as in #1813.

## Decisions
None.

## Documentation
No user-facing docs cover transit parsing internals. No mkdocs change is needed.

## Testing Strategy
The bug is reachable in the wild through ingestion, and unit tests with constructed UDTs and
buffers reproduce it exactly. In `rust/transit/tests/test_corrupt_input.rs`, each test pins
#1814:
- `parse_pod_instance_rejects_fan_out_on_tiny_object`: the issue's shape. `T0 { v: u8 }`,
  `T1` and `T2`, where `T1` and `T2` each have 16 members of the previous type at offset 0,
  size 1, parsed as `T2` against a 1-byte window. That is 528 visited members: under the
  #1813 cap of 4096 it returns `Ok`, and under the new 1-byte budget of 10 it returns `Err`,
  so this test fails before the fix.
- `parse_object_buffer_rejects_fan_out_on_tiny_objects`: the same types as a buffer of many
  2-byte objects (`[idx_of_T2, 0]` repeated) through `parse_object_buffer` returns `Err` on the
  first object.
- `read_dependencies_rejects_dependency_larger_than_declared_size`: a dependency (e.g.
  `PropertySetDependency` through the custom reader, or `StaticStringDependency`) whose
  declared size is smaller than its payload returns `Err`.
- `parse_pod_instance_accepts_dense_u8_struct`: a 64-byte UDT of 64 `u8` members parses, which
  confirms the densest legitimate layout fits the budget.
- The existing `parse_pod_instance_accepts_legitimate_nesting` and the `rust/tracing` /
  `micromegas-analytics` parse tests (`parse_corrupt_block_tests`, plus `cargo test` over
  `transit`, `tracing`, `analytics`) confirm real layouts are unaffected.

## Open Questions
None.
