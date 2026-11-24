# Bound Transit Nested-UDT Recursion Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1813

## Overview
`parse_pod_instance` recurses into a nested member whenever the member's `type_name` names a UDT
in the stream metadata. It has no depth limit and no cycle check. That metadata arrives from the
client through `/ingestion/insert_stream`, so it is untrusted. A self-referential type
(`A { m: A @ 0 }`) overflows the stack, and a stack overflow aborts the whole process
(flight-sql-srv, maintenance daemon, or monolith) on every retry. This plan bounds the recursion
so hostile metadata produces an ordinary `Err`, which the existing corrupt-block handling already
logs and skips.

## Current State
- `rust/transit/src/parser.rs:155` `parse_pod_instance(bump, udt, udts, dependencies, window)` is
  `pub`. It loops over `udt.members`, and the non-intrinsic arm (`parser.rs:220-247`) resolves the
  member type by name in `udts` and recurses on
  `&object_window[member_meta.offset..member_meta.offset + member_udt.size]`.
- The window guards from #1192 bound every slice, but they don't force the window to *shrink*.
  A nested member at offset 0 whose UDT has the parent's size gets the same window back. A
  zero-size nested UDT gets an empty window, which still passes `0 <= 0`. So recursion depth is
  bounded by nothing.
- Fan-out is also unbounded. Each UDT may list many members of another UDT at overlapping
  offsets. An acyclic chain of `D` levels with `F` members each costs `F^D` parse calls and
  arena allocations, so a few KB of metadata can exhaust CPU and memory (OOM is also an abort).
- Entry points: `read_dependencies` (`parser.rs:118`), `parse_object_buffer` (`parser.rs:313`),
  and the custom readers in `rust/tracing/src/parsing.rs` (lines 95, 129, 198, 246), which call
  `parse_pod_instance` on a fixed layout. A custom reader is never invoked from inside
  `parse_pod_instance`, so recursion stays within that one function.
- Real instrumentation types are shallow: a few levels at most (e.g. a `StringId`-style member
  inside an event), with small member counts.
- Tests: `rust/transit/tests/test_corrupt_input.rs` has `pod_udt` / `member` helpers and
  `parse_pod_instance_*` corruption cases.

## Design
Keep `parse_pod_instance`'s public signature unchanged. It becomes a thin wrapper that creates a
fresh budget and calls a private recursive worker:

```rust
const MAX_UDT_NESTING_DEPTH: usize = 32;
const MAX_UDT_NODES_PER_INSTANCE: usize = 4096;

struct ParseBudget { depth: usize, nodes_left: usize }

fn parse_pod_instance_bounded<'a>(..., budget: &mut ParseBudget) -> Result<Value<'a>>
```

- **Depth.** On entry the worker fails if `budget.depth >= MAX_UDT_NESTING_DEPTH`, with
  `bail!("corrupt metadata: udt {name} nesting exceeds {MAX} levels")`. Around the recursive
  call it increments `depth` and decrements it afterwards. That stops cycles of any length,
  including self-reference and zero-size types.
- **Node budget.** Every member visited (intrinsic, reference, or nested) decrements `nodes_left`.
  Hitting zero fails with a similar `corrupt metadata` error. That stops the `F^D` fan-out blowup
  that depth alone allows. The budget is per top-level `parse_pod_instance` call, so one large
  legitimate object can't starve its siblings.
- Both limits are far above any real type: depth of a handful, and tens of members. Exceeding
  either one means the metadata is malformed, not that a client is being stingy.

No cycle check at metadata load time. The depth limit already turns cycles into errors at the
one place they hurt, and a load-time graph walk would add a second code path for the same
property (see Trade-offs).

## Implementation Steps
1. `rust/transit/src/parser.rs`: add the two constants and `ParseBudget`. Move the body of
   `parse_pod_instance` into `parse_pod_instance_bounded`, adding the depth check on entry, the
   per-member node decrement, and a recursive call that threads `budget` with depth +1/-1.
   `parse_pod_instance` creates `ParseBudget { depth: 0, nodes_left: MAX_UDT_NODES_PER_INSTANCE }`
   and delegates.
2. `rust/transit/tests/test_corrupt_input.rs`: add regression tests (see Testing Strategy).
3. `CHANGELOG.md` is updated by the `pr` step (Unreleased → fix entry citing #1813).

## Files to Modify
- `rust/transit/src/parser.rs`
- `rust/transit/tests/test_corrupt_input.rs`
- `CHANGELOG.md`

## Trade-offs
- **Depth counter vs. visited-set cycle detection in the parser.** A visited set (type-name stack)
  rejects only true cycles. It still allows deep acyclic chains and fan-out, and costs a lookup
  per level. A depth counter is O(1) and also caps acyclic depth.
- **Load-time cycle rejection** (the issue's optional step). The parser is reachable with
  metadata from several sources (stream metadata in analytics, tests, custom readers), so a
  load-time check would not remove the need for the runtime guard. It's skipped to avoid two
  mechanisms for one property.
- **Node budget vs. depth only.** Depth only fixes the stack overflow reported in the issue, but
  leaves an exponential-fan-out OOM, which is the same abort class reached through the same
  input. The budget is one counter in the same struct, so it's included.
- **Constants vs. configuration.** These are sanity limits on malformed input, not tuning knobs,
  so they're hard-coded.

## Decisions

## Documentation
No user-facing docs describe transit parsing internals. No mkdocs change is needed.

## Testing Strategy
This is a bug reachable in the wild through the ingestion API, but every trigger is reachable by
calling `parse_pod_instance` with constructed UDTs, so unit tests reproduce it exactly. In
`rust/transit/tests/test_corrupt_input.rs`, each test pins #1813:
- `parse_pod_instance_rejects_self_referential_udt`: the issue's repro,
  `A { size: 8, members: [m: A @ 0, size 8] }` against an 8-byte window, returns `Err` (before
  the fix this aborts the test binary with a stack overflow).
- `parse_pod_instance_rejects_zero_size_self_referential_udt`: `size: 0`, member size 0, empty
  window, returns `Err`.
- `parse_pod_instance_rejects_mutually_recursive_udts`: `A → B → A` returns `Err`.
- `parse_pod_instance_rejects_exponential_fan_out`: a chain of ~10 types, each with 8
  overlapping members of the next type, returns `Err` quickly instead of doing 8^10 work.
- `parse_pod_instance_accepts_legitimate_nesting`: a 3-level nested struct with intrinsic leaves
  parses and yields the expected nested `Value::Object` values.
- `parse_object_buffer_rejects_self_referential_udt`: the same repro through
  `parse_object_buffer`, confirming the entry-point path returns `Err`.

Existing suites (`cargo test -p micromegas-transit`, and the `micromegas-analytics`
`parse_corrupt_block_tests` and `parse_block` bench build) confirm real payloads are unaffected.

## Open Questions
None.
