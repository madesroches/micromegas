# Resilient Malformed CPU Streams Plan

## Overview

Thread span blocks occasionally contain non-nested (overlapping) span events — a client-side instrumentation bug. Today this causes two problems: a phantom empty partition is silently written to the DB (masking the error forever on future queries), and the service can abort or panic due to the detached task interaction. The fix changes the data channel type to `Result<PartitionRowSet, anyhow::Error>`: callers send `Ok(row_set)` for data and `Err(e)` to abort. `write_rows_and_track_times` propagates the caller's `Err` via `msg?`, which causes `write_partition_from_rows` to return early before reaching `insert_partition`. On the thread-spans abort path the original build error is logged (`warn!`) and returned directly to the query, and the DataFusion boundary is updated to surface the full anyhow chain so the query fails with the descriptive mismatch message instead of a generic outer context.

## Current State

### Error origin

`CallTreeBuilder::on_end_thread_scope` (`rust/analytics/src/call_tree.rs:181-189`) calls `anyhow::bail!` when an End event arrives for a scope that doesn't match the stack top and isn't the root placeholder. This is the correct behavior. However, the committed (HEAD) message is `"top scope mismatch parsing thread block"` — it omits the block ID and scope names. This plan improves it to include block ID, closing scope name, and open scope name (see Implementation Steps step 4). Note: this improvement is already applied in the working tree (uncommitted) and only needs to be committed.

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

The build phase currently uses `?` directly, which drops `tx` implicitly on error. That needs to become an explicit `Err` send so the writer skips `insert_partition`, while the **original** build error is propagated to the query:

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
    Ok(row_set) => {
        tx.send(Ok(row_set)).await?;
        drop(tx);
        join_handle.await??;
        Ok(())
    }
    Err(e) => {
        warn!(
            "aborting thread-spans partition write for block {}: {e:?}",
            spec.block_ids_hash
        );
        // Poison the channel so the writer returns early and SKIPS insert_partition —
        // this is what prevents the phantom empty partition; a plain drop(tx) would
        // instead commit num_rows=0. We ignore the SendError: it can only fail if the
        // writer task already died (panic/cancel), in which case it never reached
        // insert_partition either, and the join reap below surfaces that cause.
        let _ = tx.send(Err(anyhow::anyhow!("thread-spans build aborted"))).await;
        drop(tx);
        // Reap the writer task (don't leave it detached). Its result is secondary to
        // e, but a panic or a real writer error is still worth recording.
        match join_handle.await {
            Ok(Ok(())) => {}
            Ok(Err(writer_err)) => {
                debug!("thread-spans writer task error during abort: {writer_err:?}");
            }
            Err(join_err) => {
                warn!("thread-spans writer task panicked during abort: {join_err:?}");
            }
        }
        // Propagate the original, descriptive build error to the query.
        Err(e)
    }
}
```

**Why the abort send is load-bearing (not cleanup).** The tempting simplification "just `drop(tx)` and skip the send" is wrong: dropping `tx` makes the writer's `recv()` return `None`, the loop exits, `write_rows_and_track_times` returns `Ok(None)`, and `write_partition_from_rows` proceeds straight to `finalize_partition_write(None)` + `insert_partition(num_rows=0)` — the exact phantom partition this plan exists to prevent. The poison `Err` is the only thing that forces the writer onto its early-return path.

**Why correctness holds whether the send succeeds or fails.** `insert_partition` is only reachable after the writer loop exits via channel-close (`None`), which requires *all* senders dropped. We still hold `tx` at the send point, so the writer cannot have closed-and-committed yet. Therefore: send succeeds → writer gets the poison and returns early (insert skipped); send fails → the writer task already ended *without* a normal channel close (it never reached insert either). No phantom partition is possible on either branch — which is exactly why ignoring the `SendError` is safe.

**Why we return `e` directly instead of relying on `join_handle.await??`.** On the abort path nothing is sent before the poison (the build emits a single row set at the end or aborts), so the writer is parked at `recv().await` with nothing to write — it cannot have errored on a prior message, and it cannot have completed `Ok`. The only way the send fails is a writer **panic/cancel**. Relying on `join_handle.await??` to carry the error back would, in that race, surface the *writer's* error (or a `JoinError`) instead of the informative mismatch. Returning the original `e` guarantees the query always gets the descriptive message; the `match` reaps the task so it is never detached, and surfaces a panic via `warn!` (the case the old `??` used to catch). `Ok(Err(writer_err))` on this path is essentially always just the poison sentinel echoing back, hence `debug!`.

On the happy path: writer receives `Ok(row_set)`, writes, closes the channel, calls `finalize_partition_write` + `insert_partition` as before, and `join_handle.await??` surfaces any genuine writer error.

### Why other callers don't get the abort path beyond `Ok(...)` wrapping

This plan's abort path is scoped to the thread-spans crossing-span error. The other callers get only the `Ok(...)` wrapping; none get explicit `Err` aborts.

Callers like `block_partition_spec` send multiple batches then drop `tx`; they never send `Err`. Their existing block-level errors are already logged and swallowed — the partition is committed with whatever data was collected. That behaviour is preserved exactly. No abort semantics are introduced for those callers.

`net_spans_view.rs` is a special case worth calling out: it has the *same structure* as `thread_spans_view.rs` (spawn writer at line 136, build rows with `?` at lines 178/193, await join handle at line 215), so any build `?` error there also drops `tx` and lets the detached writer commit a `num_rows=0` phantom partition. However, the *malformed-CPU-stream* trigger this plan targets does **not** fire for net spans: `NetSpanTrack::close_span` (`rust/analytics/src/net_span_tree.rs:96-116`) returns `Ok(true)` on a stack mismatch (it skips overlapping/orphan End events instead of `bail!`-ing), so a crossing net span never produces an error to abort on. The structural phantom-partition risk remains for *other* net_spans build errors (e.g. the `ensure!` on line 152 or `record_builder.finish()` on line 205), but fixing that is out of scope for this plan — it is not triggered by malformed CPU streams.

### Error propagation

```
on_end_thread_scope (bail!)
  → append_call_tree
  → build_result = Err(e)
  → poison send stops the writer (insert_partition skipped); write_partition returns Err(e) directly
  → update_partition → jit_update            (each adds .with_context(...) on top of e)
  → MaterializedView::scan  (anyhow::Error → DataFusionError::External via format!("{e:#}"))
  → df.execute_stream()
  → process_spans query  (→ query fails with the full, descriptive error chain)
```

The caller receives a query failure with the mismatch message. The service keeps running. Next time the same data is queried, `jit_update` retries because no partition record was written.

### Surfacing an informative query error

The mismatch message is the *root* of the anyhow chain, but every layer above it wraps *on top*: `append_call_tree` adds `"adding call tree to span record builder"`, `update_partition` adds `"write_partition"`, `jit_update` adds `"update_partition"`. At the DataFusion boundary in `MaterializedView::scan` (`rust/analytics/src/lakehouse/materialized_view.rs:74`) the error is currently converted with:

```rust
.map_err(|e| DataFusionError::External(e.into()))?;
```

`DataFusionError::External` surfaces the boxed error via its **default** `Display`, which shows only the *outermost* anyhow context. So today the query would come back with just `External error: update_partition` — the descriptive mismatch detail is buried in `source()` and never reaches the user. (This also means Testing Strategy bullet 3 below is *not achievable* without this change.)

Fix: flatten the full anyhow chain into the message with the alternate formatter `{e:#}`:

```rust
.map_err(|e| DataFusionError::External(format!("{e:#}").into()))?;
```

which yields e.g. `External error: update_partition: write_partition: adding call tree to span record builder: top scope mismatch in block <id>: closing 'A' but 'B' is open`. (`String` has a `From` impl into `Box<dyn Error + Send + Sync>`, so `format!(...).into()` is valid; use `DataFusionError::Execution(format!("{e:#}"))` instead if the `External error:` prefix is unwanted.) This is the shared `scan` path used by **all** views, so it improves error reporting everywhere — a slightly broader touch than the thread-spans-only edits, but generally desirable.

## Implementation Steps

1. **`write_partition.rs`**:
   - Change `write_partition_from_rows` parameter: `Receiver<PartitionRowSet>` → `Receiver<Result<PartitionRowSet, anyhow::Error>>`
   - In `write_rows_and_track_times`, change loop body: `while let Some(row_set) = rb_stream.recv()` → `while let Some(msg) = rb_stream.recv() { let row_set = msg?; ... }`
   - No other changes needed.

2. **`thread_spans_view.rs`** — wrap the build loop in an inner async block; replace the trailing `tx.send(PartitionRowSet { ... }).await?` with the `match build_result { Ok → send + join.await??, Err → warn!, poison-send, reap-and-log join, return Err(e) }` pattern shown above. Ensure `warn!` and `debug!` are imported via `micromegas_tracing::prelude::*` (the file already uses `info!`).

3. **Five remaining callers** — wrap each `tx.send(<row_set expr>)` argument in `Ok(...)`. Only `block_partition_spec.rs` sends a bare `row_set` variable; the others send constructed values, so the wrap applies to the constructor/struct-literal expression (e.g. `tx.send(Ok(PartitionRowSet::new(...)))`, `tx.send(Ok(PartitionRowSet { ... }))`):
   - `rust/analytics/src/lakehouse/net_spans_view.rs` (`PartitionRowSet { ... }` struct literal)
   - `rust/analytics/src/lakehouse/sql_partition_spec.rs` (`PartitionRowSet::new(...)`)
   - `rust/analytics/src/lakehouse/block_partition_spec.rs` (bare `row_set`)
   - `rust/analytics/src/lakehouse/merge.rs` (`PartitionRowSet::new(...)`)
   - `rust/analytics/src/lakehouse/metadata_partition_spec.rs` (`PartitionRowSet::new(...)`)

4. **`call_tree.rs`** — improve the `on_end_thread_scope` mismatch diagnostic. (Already applied in the working tree, uncommitted — verify it matches the below and commit it; no further edit needed.) Replace the committed `anyhow::bail!("top scope mismatch parsing thread block")` with a message that includes the block ID, the closing scope name, and the open scope name:

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

5. **`materialized_view.rs`** — at the `scan` jit_update boundary (line 74), change `DataFusionError::External(e.into())` to `DataFusionError::External(format!("{e:#}").into())` so the full anyhow chain (including the mismatch detail) reaches the query error. See "Surfacing an informative query error".

6. Run `cargo fmt && cargo clippy --workspace -- -D warnings && cargo test`.

## Files to Modify

- `rust/analytics/src/lakehouse/write_partition.rs`
- `rust/analytics/src/lakehouse/thread_spans_view.rs`
- `rust/analytics/src/lakehouse/materialized_view.rs` (flatten anyhow chain into the query error at the `scan` boundary — see step 5)
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
- Confirm `process_spans(...)` returns a DataFusion error whose message contains the mismatch detail when a bad stream is included. This depends on the `{e:#}` flattening at the `materialized_view.rs` `scan` boundary (step 5); without it the query surfaces only the outermost context (`update_partition`) and this assertion fails.
