# Cargo.toml Alphabetical Ordering Fix Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1254

## Overview
Two dependency pairs in `rust/Cargo.toml`'s `[workspace.dependencies]` block violate the project's alphabetical-ordering rule (per CLAUDE.md / AI_GUIDELINES.md). Swap each pair so the block is fully alphabetical. This is a trivial reordering with no behavior change.

## Current State
In `rust/Cargo.toml`, `[workspace.dependencies]`:
- Line 74: `rand = "0.9"` precedes line 75: `quote = "1.0"` — `quote` < `rand`, so `quote` should come first.
- Line 87: `thrift = "0.17"` precedes line 88: `thread-id = "4.0"` — `thread-id` < `thrift` (both start `thr`, then `e` < `i`), so `thread-id` should come first.

## Design
Pure line-swap within the alphabetized dependency block. No version, feature, or value changes — only line order.

Target result:
```toml
quote = "1.0"
rand = "0.9"
...
thread-id = "4.0"
thrift = "0.17"
```

## Implementation Steps
1. In `rust/Cargo.toml`, swap lines 74–75 so `quote = "1.0"` precedes `rand = "0.9"`.
2. Swap lines 87–88 so `thread-id = "4.0"` precedes `thrift = "0.17"`.

## Files to Modify
- `rust/Cargo.toml`

## Testing Strategy
- `cargo build` (from `rust/`) still succeeds — the acceptance criterion. Because only the order of two independent workspace-dependency declarations changes, no other verification is needed.

## Trade-offs
None. The ordering rule is unambiguous and the fix is mechanical.

## Documentation
None. No user-facing or project documentation is affected.

## Open Questions
None.
