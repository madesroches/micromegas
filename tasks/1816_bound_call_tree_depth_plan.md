# Bound Call-Tree Depth Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1816

## Overview
`CallTreeBuilder` pushes a node for every begin-scope event that has no matching end, and the
tree it produces has no depth limit. Two code paths recurse over that tree: the pre-order
traversal that feeds `thread_spans` rows, and the compiler-generated `Drop` for `CallTreeNode`.
A block carrying many unmatched begin events (a few bytes each) builds a tree deep enough to
overflow the stack. A stack overflow aborts the whole process (flight-sql-srv, maintenance
daemon, or monolith), not just the query. This plan caps the depth with an ordinary `Err` and
makes the traversal and the drop iterative, so no tree shape can abort the process.

## Current State
- `rust/analytics/src/call_tree.rs:18` `CallTreeNode` holds `children: Vec<CallTreeNode>` and
  derives `Debug`. There is no manual `Drop`, so dropping a chain of depth `D` recurses `D`
  frames.
- `CallTreeBuilder::on_begin_thread_scope` (`call_tree.rs:154`) filters on the span limit and the
  time range, then pushes onto `self.stack` with no depth check. `on_end_thread_scope` pops the
  top and attaches it to the new top via `add_child_to_top`. `finish()` (`call_tree.rs:90`)
  folds the unclosed stack into a parent/child chain. Each node's subtree was built while that
  node sat on the stack, so the tree's depth is bounded by the maximum `stack.len()` reached
  (plus at most one synthetic thread root).
- `on_end_thread_scope` already returns `Err` on malformed input (`top scope mismatch`,
  `call_tree.rs:222`). That error goes through `make_call_tree` → `append_call_tree` in
  `rust/analytics/src/lakehouse/thread_spans_view.rs:98` to `write_partition`, whose
  `build_result` `Err` arm (`thread_spans_view.rs:250`) logs a warning and aborts that
  partition's writer. A depth error takes the same path.
- `rust/analytics/src/span_table.rs:204` `for_each_node_in_tree` is a recursive pre-order walk,
  called from `SpanRecordBuilder::append_call_tree` (`span_table.rs:145`). Row order matters:
  `ensure_begin_non_decreasing` and `max_sort_key_time` in `write_partition` assume pre-order,
  with each node's children in `Vec` order. `test_preorder_rows_are_non_decreasing_and_last_row_is_max`
  in `rust/analytics/tests/call_tree_tests.rs` pins that order.
- No code outside `call_tree.rs` constructs `CallTreeNode` or moves fields out of it, so adding
  `impl Drop` breaks no destructuring.

## Design
### 1. Depth cap in the builder
```rust
/// Maximum nesting depth of a call tree. ...
pub const MAX_CALL_TREE_DEPTH: usize = 4096;
```
In `on_begin_thread_scope`, after the limit and time-range filters (so only a node that would
actually be pushed counts), and before the push:
```rust
if self.stack.len() >= MAX_CALL_TREE_DEPTH {
    anyhow::bail!("call tree depth exceeds {MAX_CALL_TREE_DEPTH} in block {block_id}");
}
```
That is the only place `stack` grows by more than one node. `add_child_to_top` pushes only onto
an empty stack, so it never deepens the tree past the cap.

The cap is set far above real instrumented call depth (dozens of levels, occasionally a few
hundred for instrumented recursion), so it rejects only malformed or hostile input.

### 2. Iterative traversal
Replace the body of `for_each_node_in_tree` with an explicit stack of
`(&CallTreeNode, parent_id, depth)`. Pop a node, call `process_node`, then push its children in
**reverse** so they pop in `Vec` order. That reproduces today's pre-order exactly. The signature
stays the same.

### 3. Iterative drop
```rust
impl Drop for CallTreeNode {
    fn drop(&mut self) {
        let mut pending = std::mem::take(&mut self.children);
        while let Some(mut node) = pending.pop() {
            pending.append(&mut node.children);
        }
    }
}
```
Each popped `node` is dropped at the end of its loop iteration with an empty `children`, so the
nested `Drop` returns immediately. Peak memory is the `pending` vector, which holds at most the
tree's node count. This covers every owner of nodes: `CallTree::call_tree_root`, the builder's
`stack` when it is dropped on error, and test-built trees.

## Implementation Steps
1. `rust/analytics/src/call_tree.rs`: add `MAX_CALL_TREE_DEPTH`, the depth check in
   `on_begin_thread_scope` (rename `_block_id` to `block_id` there), and `impl Drop for CallTreeNode`.
2. `rust/analytics/src/span_table.rs`: rewrite `for_each_node_in_tree` iteratively, as in
   Design §2.
3. `rust/analytics/tests/call_tree_tests.rs`: add the tests below.
4. `CHANGELOG.md` is updated by the `pr` step (Unreleased → fix entry citing #1816).

## Files to Modify
- `rust/analytics/src/call_tree.rs`
- `rust/analytics/src/span_table.rs`
- `rust/analytics/tests/call_tree_tests.rs`
- `CHANGELOG.md`

## Trade-offs
- **Cap only vs. cap plus iterative paths.** A cap alone low enough to make recursion safe on a
  tokio worker stack would have to account for frame sizes that vary with build profile and
  inlining. Iterative traversal and drop remove that coupling, and the cap can stay generous.
  This is the issue's suggested fix.
- **Iterative only, no cap.** That would leave the derived `Debug` and any future recursive
  consumer exposed, and it would let `depth` grow without bound. The cap is one comparison per
  begin event.
- **Error vs. truncation past the cap.** Silently dropping spans past the cap would produce a
  partition that looks valid but is wrong. An `Err` follows the existing `top scope mismatch`
  precedent and surfaces the corrupt input.

## Testing Strategy
Unit tests in `rust/analytics/tests/call_tree_tests.rs`, no DB:
- **Depth cap.** Feed `MAX_CALL_TREE_DEPTH` unmatched begins: all succeed. The next begin
  returns `Err` whose message contains `call tree depth exceeds`. Also confirm that a begin
  before `begin_range_ns`, which is filtered and never pushed, does not trip the cap at full
  depth.
- **Deep drop (issue reproduction).** Build a 100,000-deep `CallTreeNode` chain by hand
  (iteratively, using the public fields) and drop it on a `std::thread` spawned with a small stack
  (e.g. 256 KiB). Join the thread and assert it completed. Before the fix, the process aborts.
- **Deep traversal.** Build a 100,000-deep chain by hand inside a `CallTree` (it bypasses the
  builder cap on purpose, because the traversal must not rely on the cap). Run
  `SpanRecordBuilder::append_call_tree` on a small-stack thread, then assert the row count and
  that the last row's `depth` equals the chain depth and its `parent` equals its predecessor's
  `id`.
- **Order.** The existing `test_preorder_rows_are_non_decreasing_and_last_row_is_max` and the
  other `append_call_tree` tests pin pre-order with children in order. They must still pass
  unchanged.
