# Resilient Malformed CPU Streams Plan

## Overview

Thread span blocks occasionally contain non-nested (overlapping) span events — a client-side instrumentation bug. Today this causes two problems: a phantom empty partition is silently written to the DB (masking the error forever on future queries), and the service can abort or panic due to the detached task interaction. The fix changes the data channel type to `Result<PartitionRowSet, anyhow::Error>`: callers send `Ok(row_set)` for data and `Err(e)` to abort. `write_rows_and_track_times` propagates the caller's `Err` via `msg?`, which causes `write_partition_from_rows` to return early before reaching `insert_partition`.

## Current State

### Error origin

`CallTreeBuilder::on_end_thread_scope` (`rust/analytics/src/call_tree.rs:181-189`) calls `anyhow::bail!` when an End event arrives for a scope that doesn't match the stack top and isn't the root placeholder. This is the correct behavior. However, the committed message is `"top scope mismatch parsing thread block"` — it omits the block ID and scope names. This plan improves it to include block ID, closing scope name, and open scope name (see Implementation Steps step 4).

### Problem — Phantom empty partition + detached task

`write_partition` (`rust/analytics/src/lakehouse/thread_spans_view.rs:106-180`) spawns `write_partition_from_rows` via `spawn_with_context` at **line 128**, before any data is built. If `append_call_tree` fails at line 150 or 163, `write_partition` returns `Err` via `?`, dropping both `tx` (channel sender) and `join_handle` without awaiting it.

Two consequences:

1. **Phantom partition**: The detached `write_partition_from_rows` task sees `None` from `rb_stream.recv()` (sender dropped), calls `finalize_partition_write` with `event_time_range = None`, then calls `insert_partition` — writing a `num_rows=0` record to `lakehouse_partitions`. `is_jit_partition_up_to_date` finds this record on every subsequent query and returns `true`, permanently skipping those blocks. The malformed stream is buried.

2. **Detached task / crash path**: Dropping a `JoinHandle` without awaiting it leaves the task running uncontrolled. If the task panics (e.g. due to an unexpected DB error), the behaviour depends on the Tokio runtime configuration and can produce an abort. The task also races against the DB and object store in ways that are hard to reason about.

The error from `bail!` would otherwise propagate cleanly all the way to the `process_spans` caller — making the query fail with a descriptive message — but the phantom partition means it will *never be retried* and future queries silently return empty data for that thread.

`write_partition_from_rows` is shared by six callers (thread_spans_view, net_spans_view, sql_partition_spec, block_partition_spec, merge, metadata_partition_spec), all with the same structural risk.

## Design

### Change the channel type to `Result<PartitionRowSet, anyhow::Error>`

The existing `?` on `write_rows_and_track_times(...)` in `write_partition_from_rows` already causes early return before `finalize_partition_write` and `insert_partition` when the writer itself encounters an error. We just need that same short-circuit to fire when the *caller* signals an abort.

By changing the channel item type to `Result<PartitionRowSet, anyhow::Error>`, the caller can explicitly send `Err(e)` to abort. Inside `write_rows_and_track_times`, `msg?` propagates that error out, and the existing `?` chain does the rest — no structural change to `write_partition_from_rows` is needed.

```
[callers]  tx.send(Ok(row_set))  — normal data
           tx.send(Err(e))       — abort: writer returns Err(e), skips insert_partition
           drop(tx)              — channel closed normally: writer commits empty partition (existing behaviour)
```

### Changes to `write_rows_and_track_times` (private fn in `write_partition.rs`)

```rust
// Before:
async fn write_rows_and_track_times(
    rb_stream: &mut Receiver<PartitionRowSet>,
    ...
) -> Result<Option<TimeRange>> {
    while let Some(row_set) = rb_stream.recv().await {
        // write row_set
    }
    ...
}

// After:
async fn write_rows_and_track_times(
    rb_stream: &mut Receiver<Result<PartitionRowSet, anyhow::Error>>,
    ...
) -> Result<Option<TimeRange>> {
    while let Some(msg) = rb_stream.recv().await {
        let row_set = msg?;   // propagates caller's Err; writer returns early, skips insert_partition
        // write row_set (unchanged)
    }
    ...
}
```

`write_partition_from_rows` itself is otherwise unchanged. The existing `?` after `write_rows_and_track_times(...)` already skips `finalize_partition_write` and `insert_partition` on any error.

### Changes to callers — success paths (all six)

Replace `tx.send(row_set)` with `tx.send(Ok(row_set))`. Callers that legitimately close the channel without sending (producing an empty partition) need no change to their closing logic — `drop(tx)` still produces the "empty but committed" behaviour.

### Changes to `thread_spans_view.rs` — explicit abort on build error

The build phase currently uses `?` directly, which drops `tx` implicitly on error. That needs to become an explicit `Err(e)` send so the writer can propagate the error back:

```rust
// Wrap build phase in an inner async block to collect the result
let build_result: Result<PartitionRowSet> = async {
    let mut record_builder = SpanRecordBuilder::with_capacity(nb_events / 2);
    // ... same block-grouping logic with ? ...
    let rows = record_builder.finish().with_context(|| "record_builder.finish()")?;
    Ok(PartitionRowSet {
        rows_time_range: TimeRange::new(min_time_row, max_time_row),
        rows,
    })
}.await;

match build_result {
    Ok(row_set) => { tx.send(Ok(row_set)).await?; }
    Err(e)      => { let _ = tx.send(Err(e)).await; }
}
drop(tx);
join_handle.await??;   // writer's Err(e) propagates here; write_partition returns Err(e)
Ok(())
```

On the error path: `tx.send(Err(e))` causes `write_rows_and_track_times` to return `Err(e)`, which causes `write_partition_from_rows` to return `Err(e)`, which is captured by the JoinHandle and propagated by `join_handle.await??`. `insert_partition` is never called.

On the happy path: writer receives `Ok(row_set)`, writes, closes the channel, calls `finalize_partition_write` + `insert_partition` as before.

### Why other callers don't get the abort path beyond `Ok(...)` wrapping

This plan's abort path is scoped to the thread-spans crossing-span error. The other callers get only the `Ok(...)` wrapping; none get explicit `Err` aborts.

Callers like `block_partition_spec` send multiple batches then drop `tx`; they never send `Err`. Their existing block-level errors are already logged and swallowed — the partition is committed with whatever data was collected. That behaviour is preserved exactly. No abort semantics are introduced for those callers.

`net_spans_view.rs` is a special case worth calling out: it has the *same structure* as `thread_spans_view.rs` (spawn writer at line 136, build rows with `?` at lines 178/193, await join handle at line 215), so any build `?` error there also drops `tx` and lets the detached writer commit a `num_rows=0` phantom partition. However, the *malformed-CPU-stream* trigger this plan targets does **not** fire for net spans: `NetSpanTrack::close_span` (`rust/analytics/src/net_span_tree.rs:96-116`) returns `Ok(true)` on a stack mismatch (it skips overlapping/orphan End events instead of `bail!`-ing), so a crossing net span never produces an error to abort on. The structural phantom-partition risk remains for *other* net_spans build errors (e.g. the `ensure!` on line 152 or `record_builder.finish()` on line 205), but fixing that is out of scope for this plan — it is not triggered by malformed CPU streams.

### Error propagation (unchanged, already correct)

```
on_end_thread_scope (bail!)
  → append_call_tree
  → build_result = Err(e)
  → tx.send(Err(e))  →  write_rows_and_track_times returns Err(e)
                     →  write_partition_from_rows returns Err(e)  [insert_partition skipped]
                     →  join_handle.await?? propagates Err(e)
  → write_partition returns Err(e)
  → update_partition → jit_update
  → MaterializedView::scan  (→ DataFusionError::External)
  → df.execute_stream()
  → process_spans query  (→ query fails with descriptive error message)
```

The caller receives a query failure with the mismatch message. The service keeps running. Next time the same data is queried, `jit_update` retries because no partition record was written.

## Implementation Steps

1. **`write_partition.rs`**:
   - Change `write_partition_from_rows` parameter: `Receiver<PartitionRowSet>` → `Receiver<Result<PartitionRowSet, anyhow::Error>>`
   - In `write_rows_and_track_times`, change loop body: `while let Some(row_set) = rb_stream.recv()` → `while let Some(msg) = rb_stream.recv() { let row_set = msg?; ... }`
   - No other changes needed.

2. **`thread_spans_view.rs`** — wrap the build loop in an inner async block; replace the trailing `tx.send(PartitionRowSet { ... }).await?` with the `match build_result { Ok → tx.send(Ok(...)), Err → tx.send(Err(...)) }` pattern shown above.

3. **Five remaining callers** — change each `tx.send(row_set)` to `tx.send(Ok(row_set))`:
   - `rust/analytics/src/lakehouse/net_spans_view.rs`
   - `rust/analytics/src/lakehouse/sql_partition_spec.rs`
   - `rust/analytics/src/lakehouse/block_partition_spec.rs`
   - `rust/analytics/src/lakehouse/merge.rs`
   - `rust/analytics/src/lakehouse/metadata_partition_spec.rs`

4. **`call_tree.rs`** — improve the `on_end_thread_scope` mismatch diagnostic. Replace the committed `anyhow::bail!("top scope mismatch parsing thread block")` with a message that includes the block ID, the closing scope name, and the open scope name:

   ```rust
   let ending_name = self.scopes.get(&hash).map_or("?", |s| s.name.as_str());
   let open_name = self
       .scopes
       .get(&old_top.hash)
       .map_or("?", |s| s.name.as_str());
   anyhow::bail!(
       "top scope mismatch in block {_block_id}: closing '{ending_name}' but '{open_name}' is open"
   );
   ```

5. Run `cargo fmt && cargo clippy --workspace -- -D warnings && cargo test`.

## Files to Modify

- `rust/analytics/src/lakehouse/write_partition.rs`
- `rust/analytics/src/lakehouse/thread_spans_view.rs`
- `rust/analytics/src/lakehouse/net_spans_view.rs`
- `rust/analytics/src/lakehouse/sql_partition_spec.rs`
- `rust/analytics/src/lakehouse/block_partition_spec.rs`
- `rust/analytics/src/lakehouse/merge.rs`
- `rust/analytics/src/lakehouse/metadata_partition_spec.rs`
- `rust/analytics/src/call_tree.rs` (improve `on_end_thread_scope` mismatch diagnostic message — see step 4)

## Trade-offs

**Query fails entirely vs. partial results per thread**: `process_spans` fails as soon as any thread stream hits a mismatch. Per-thread isolation (log-and-skip) was considered but rejected: a failing query with a clear error message is the right signal that the upstream instrumentation is broken.

**Other callers that implicitly drop tx on error**: Callers that return early via `?` before sending anything leave the channel closed without an `Err` message. The writer treats this as "empty but committed" and calls `insert_partition`. This is the pre-existing behaviour and is intentional for streaming callers where partial results are acceptable. Only `thread_spans_view.rs` requires the explicit abort path *for the malformed-CPU-stream error this plan targets*. `net_spans_view.rs` shares the same structural phantom-partition risk on other build errors (see "Why other callers don't get the abort path"), but the crossing-span trigger does not fire there because net span parsing skips mismatches (`close_span` returns `Ok(true)`) rather than `bail!`-ing, so that risk is left as-is and out of scope here.

## Testing Strategy

- Unit test in `rust/analytics/tests/`: construct a `CallTreeBuilder`, feed it crossing spans (BeginA → BeginB → EndA sequence), and assert that processing returns `Err`. With the improved diagnostic from step 4, assert the message contains both scope names and the block ID.
- Confirm no row is inserted into `lakehouse_partitions` for a stream whose `jit_update` returns `Err` (i.e., `is_jit_partition_up_to_date` returns `false` on the next call for those blocks).
- Confirm `process_spans(...)` returns a DataFusion execution error whose message contains the mismatch detail when a bad stream is included.
