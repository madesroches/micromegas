# Resilient Partition Writes Plan

## Overview

Thread span blocks occasionally contain non-nested (overlapping) span events — a client-side
instrumentation bug. Today this causes two problems: a phantom empty partition is silently written
to the DB (masking the error forever on future queries), and the service can abort or panic due to
the detached task interaction. The root cause is structural and shared by multiple callers of
`write_partition_from_rows`.

The fix changes the data channel type to `Result<PartitionRowSet, anyhow::Error>`: callers send
`Ok(row_set)` for data and `Err(e)` to abort. `write_rows_and_track_times` propagates the caller's
`Err` via `msg?`, which causes `write_partition_from_rows` to return early before reaching
`insert_partition`. On the abort path the original build error is logged (`warn!`) and returned
directly to the query, and the DataFusion boundary is updated to surface the full anyhow chain so
the query fails with the descriptive mismatch message instead of a generic outer context.

Three callers receive explicit abort paths: `thread_spans_view.rs`, `net_spans_view.rs`, and
`merge.rs`. The remaining three callers (`sql_partition_spec.rs`, `block_partition_spec.rs`,
`metadata_partition_spec.rs`) receive only the `Ok(...)` wrapping — their empty-on-error behaviour
is intentional and must be preserved.

## Current State

### Error origin

`CallTreeBuilder::on_end_thread_scope` (`rust/analytics/src/call_tree.rs:181-189`) calls
`anyhow::bail!` when an End event arrives for a scope that doesn't match the stack top and isn't
the root placeholder. This is the correct behavior. However, the committed (HEAD) message is
`"top scope mismatch parsing thread block"` — it omits the block ID and scope names. This plan
improves it to include block ID, closing scope name, and open scope name (see Implementation Steps
step 4). Note: this improvement is already applied in the working tree (uncommitted) and only needs
to be committed.

### Problem — Phantom empty partition + detached task

`write_partition` (`rust/analytics/src/lakehouse/thread_spans_view.rs:106-180`) spawns
`write_partition_from_rows` via `spawn_with_context` at **line 128**, before any data is built.
If `append_call_tree` fails at line 150 or 163, `write_partition` returns `Err` via `?`, dropping
both `tx` (channel sender) and `join_handle` without awaiting it.

Two consequences:

1. **Phantom partition**: The detached `write_partition_from_rows` task sees `None` from
   `rb_stream.recv()` (sender dropped), calls `finalize_partition_write` with
   `event_time_range = None`, then calls `insert_partition` — writing a `num_rows=0` record to
   `lakehouse_partitions`. `is_jit_partition_up_to_date` finds this record on every subsequent
   query and returns `true`, permanently skipping those blocks. The malformed stream is buried.

2. **Detached task / crash path**: Dropping a `JoinHandle` without awaiting it leaves the task
   running uncontrolled. If the task panics (e.g. due to an unexpected DB error), the behaviour
   depends on the Tokio runtime configuration and can produce an abort. The task also races against
   the DB and object store in ways that are hard to reason about.

The error from `bail!` would otherwise propagate cleanly all the way to the `process_spans` caller
— making the query fail with a descriptive message — but the phantom partition means it will
*never be retried* and future queries silently return empty data for that thread.

`write_partition_from_rows` is shared by six callers (thread_spans_view, net_spans_view,
sql_partition_spec, block_partition_spec, merge, metadata_partition_spec), all with the same
structural risk.

### Callers assessed

| Caller | Phantom possible? | Gets abort path? | Reasoning |
|--------|:-----------------:|:----------------:|-----------|
| `thread_spans_view.rs` | Yes | **Yes** | `bail!` on mismatch after writer spawned; no send before failure |
| `net_spans_view.rs` | Yes | **Yes** | `ensure!` + `append_net_span_tree().await?` + `record_builder.finish()?` all fire after writer spawned, before any send |
| `merge.rs` | Yes | **Yes** | stream-read errors mid-loop fire after writer spawned; partial Parquet already written but `insert_partition` must be skipped |
| `sql_partition_spec.rs` | Intentional | No | Empty-on-error is documented design; comment explicitly allows empty `record_count` |
| `block_partition_spec.rs` | No | No | Explicit empty case: early `drop(tx)` + immediate join before return |
| `metadata_partition_spec.rs` | Mostly intentional | No | SQL failure → empty partition is acceptable; `rows_to_record_batch` failure is a minor edge case left as-is |

## Design

### Change the channel type to `Result<PartitionRowSet, anyhow::Error>`

The existing `?` on `write_rows_and_track_times(...)` in `write_partition_from_rows` already causes
early return before `finalize_partition_write` and `insert_partition` when the writer itself
encounters an error. We just need that same short-circuit to fire when the *caller* signals an
abort.

By changing the channel item type to `Result<PartitionRowSet, anyhow::Error>`, the caller can
explicitly send `Err(e)` to abort. Inside `write_rows_and_track_times`, `msg?` propagates that
error out, and the existing `?` chain does the rest — no structural change to
`write_partition_from_rows` is needed.

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

`write_partition_from_rows` itself is otherwise unchanged. The existing `?` after
`write_rows_and_track_times(...)` already skips `finalize_partition_write` and `insert_partition`
on any error.

### Changes to callers — success paths (all six)

Replace `tx.send(row_set)` with `tx.send(Ok(row_set))`. Callers that legitimately close the
channel without sending (producing an empty partition) need no change to their closing logic —
`drop(tx)` still produces the "empty but committed" behaviour.

### Abort path pattern (shared by thread_spans, net_spans, merge)

```rust
match build_result {
    Ok(row_set_or_none) => {
        // send if non-empty, then drop(tx) and join_handle.await??
    }
    Err(e) => {
        warn!("aborting <view> partition write for <id>: {e:?}");
        // Poison the channel so the writer returns early and SKIPS insert_partition.
        // A plain drop(tx) would instead commit num_rows=0 — the phantom this plan prevents.
        // Ignoring SendError is safe: if the send fails, the writer task already ended without
        // a normal channel close (it never reached insert_partition either).
        let _ = tx.send(Err(anyhow::anyhow!("<view> build aborted"))).await;
        drop(tx);
        // Reap the writer task; surface panics but don't let them mask the build error.
        match join_handle.await {
            Ok(Ok(())) => {}
            Ok(Err(writer_err)) => {
                debug!("<view> writer task error during abort: {writer_err:?}");
            }
            Err(join_err) => {
                warn!("<view> writer task panicked during abort: {join_err:?}");
            }
        }
        Err(e)
    }
}
```

**Why the abort send is load-bearing (not cleanup).** Dropping `tx` makes the writer's `recv()`
return `None`, the loop exits, `write_rows_and_track_times` returns `Ok(None)`, and
`write_partition_from_rows` proceeds straight to `finalize_partition_write(None)` +
`insert_partition(num_rows=0)` — the exact phantom partition this plan exists to prevent. The
poison `Err` is the only thing that forces the writer onto its early-return path.

**Why correctness holds whether the send succeeds or fails.** `insert_partition` is only reachable
after the writer loop exits via channel-close (`None`), which requires *all* senders dropped. We
still hold `tx` at the send point, so the writer cannot have closed-and-committed yet. Therefore:
send succeeds → writer gets the poison and returns early (insert skipped); send fails → the writer
task already ended *without* a normal channel close (it never reached insert either). No phantom
partition is possible on either branch.

**Why we return `e` directly instead of relying on `join_handle.await??`.** Returning the original
`e` guarantees the query always gets the descriptive message; the `match` reaps the task so it is
never detached, and surfaces a panic via `warn!`. `Ok(Err(writer_err))` on this path is essentially
always just the poison sentinel echoing back, hence `debug!`.

### Changes to `thread_spans_view.rs` — explicit abort on build error

Wrap the build phase in an inner async block to collect the result:

```rust
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
            "aborting thread-spans partition write for block {:?}: {e:?}",
            spec.block_ids_hash
        );
        let _ = tx.send(Err(anyhow::anyhow!("thread-spans build aborted"))).await;
        drop(tx);
        match join_handle.await {
            Ok(Ok(())) => {}
            Ok(Err(writer_err)) => {
                debug!("thread-spans writer task error during abort: {writer_err:?}");
            }
            Err(join_err) => {
                warn!("thread-spans writer task panicked during abort: {join_err:?}");
            }
        }
        Err(e)
    }
}
```

### Changes to `net_spans_view.rs` — explicit abort on build error

The writer is spawned at line 136. The `bail!` at line 129 fires *before* channel creation and
is safe. The abort path covers: `ensure!` at line 152, both `append_net_span_tree().await?` calls
(lines 178/193), and `record_builder.finish()?` at line 204.

Wrap the build phase in an inner async block that returns `Result<Option<PartitionRowSet>>`:

```rust
let build_result: Result<Option<PartitionRowSet>> = async {
    // ensure! (stream validation)
    // loop over blocks, append_net_span_tree calls with ?
    // record_builder.finish()?
    if nb_rows > 0 {
        Ok(Some(PartitionRowSet { rows_time_range, rows }))
    } else {
        Ok(None)
    }
}.await;

match build_result {
    Ok(Some(row_set)) => {
        tx.send(Ok(row_set)).await?;
        drop(tx);
        join_handle.await??;
        Ok(())
    }
    Ok(None) => {
        drop(tx);
        join_handle.await??;
        Ok(())
    }
    Err(e) => {
        warn!(
            "aborting net-spans partition write for block {:?}: {e:?}",
            spec.block_ids_hash
        );
        let _ = tx.send(Err(anyhow::anyhow!("net-spans build aborted"))).await;
        drop(tx);
        match join_handle.await {
            Ok(Ok(())) => {}
            Ok(Err(writer_err)) => {
                debug!("net-spans writer task error during abort: {writer_err:?}");
            }
            Err(join_err) => {
                warn!("net-spans writer task panicked during abort: {join_err:?}");
            }
        }
        Err(e)
    }
}
```

### Changes to `merge.rs` — explicit abort on stream error

Unlike thread/net spans, merge sends batches incrementally as it reads from the stream. A
mid-stream failure means some Parquet has already been written to object storage; preventing
`insert_partition` still protects correctness (the orphaned object-store files are unreferenced
and don't affect queries). Wrap the loop body in an inner async block returning `Result<()>`:

```rust
let stream_result: Result<()> = async {
    while let Some(rb_res) = merged_stream.next().await {
        let rb = rb_res.with_context(|| "receiving record_batch from stream")?;
        let event_time_range = compute_time_bounds
            .get_time_bounds(ctx.read_batch(rb.clone()).with_context(|| "read_batch")?)
            .await?;
        tx.send(Ok(PartitionRowSet::new(event_time_range, rb)))
            .await
            .with_context(|| "sending partition row set")?;
    }
    Ok(())
}.await;

match stream_result {
    Ok(()) => {
        drop(tx);
        join_handle.await??;
        Ok(())
    }
    Err(e) => {
        warn!("aborting merge partition write for {desc}: {e:?}");
        let _ = tx.send(Err(anyhow::anyhow!("merge stream aborted"))).await;
        drop(tx);
        match join_handle.await {
            Ok(Ok(())) => {}
            Ok(Err(writer_err)) => {
                debug!("merge writer task error during abort: {writer_err:?}");
            }
            Err(join_err) => {
                warn!("merge writer task panicked during abort: {join_err:?}");
            }
        }
        Err(e)
    }
}
```

### Why the three remaining callers don't get the abort path

**`sql_partition_spec.rs`**: A comment at the top of the write block explicitly states that an
empty `record_count` is allowed. If `df.execute_stream()` fails before any send, committing an
empty partition is intentional — it records that this spec was evaluated and produced nothing.

**`block_partition_spec.rs`**: The empty case is handled explicitly with an immediate `drop(tx)` +
`join_handle.await??` before returning, not an accidental implicit drop. No structural risk.

**`metadata_partition_spec.rs`**: SQL failure → empty partition is acceptable per design. The
`rows_to_record_batch` failure edge case (row count > 0 but finish fails) is left as-is — the
window is narrow and the existing behaviour is tolerable.

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

The caller receives a query failure with the mismatch message. The service keeps running. Next time
the same data is queried, `jit_update` retries because no partition record was written.

### Surfacing an informative query error

The mismatch message is the *root* of the anyhow chain, but every layer above it wraps *on top*:
`append_call_tree` adds `"adding call tree to span record builder"`, `update_partition` adds
`"write_partition"`, `jit_update` adds `"update_partition"`. At the DataFusion boundary in
`MaterializedView::scan` (`rust/analytics/src/lakehouse/materialized_view.rs:74`) the error is
currently converted with:

```rust
.map_err(|e| DataFusionError::External(e.into()))?;
```

`DataFusionError::External` surfaces the boxed error via its **default** `Display`, which shows
only the *outermost* anyhow context. So today the query would come back with just
`External error: update_partition` — the descriptive mismatch detail is buried in `source()` and
never reaches the user.

Fix: flatten the full anyhow chain into the message with the alternate formatter `{e:#}`:

```rust
.map_err(|e| DataFusionError::External(format!("{e:#}").into()))?;
```

which yields e.g.
`External error: update_partition: write_partition: adding call tree to span record builder: top scope mismatch in block <id>: closing 'A' but 'B' is open`.
(`String` has a `From` impl into `Box<dyn Error + Send + Sync>`, so `format!(...).into()` is valid;
use `DataFusionError::Execution(format!("{e:#}"))` instead if the `External error:` prefix is
unwanted.) This is the shared `scan` path used by **all** views, so it improves error reporting
everywhere.

## Implementation Steps — COMPLETED 2026-06-03

1. ✅ **`write_partition.rs`**: Changed receiver type to `Receiver<Result<PartitionRowSet, anyhow::Error>>` on both `write_partition_from_rows` and `write_rows_and_track_times`; changed loop body to `while let Some(msg) = ... { let row_set = msg?; ... }`; made `write_rows_and_track_times` `pub`.

2. ✅ **`thread_spans_view.rs`**: Wrapped build loop in inner async block returning `Result<PartitionRowSet>`; replaced send with match pattern (abort path: warn!, poison-send, reap join, return Err(e)).

3. ✅ **`net_spans_view.rs`**: Wrapped build phase in inner async block returning `Result<Option<PartitionRowSet>>`; applied three-arm match.

4. ✅ **`merge.rs`**: Wrapped streaming loop in inner async block returning `Result<()>`; applied two-arm match; simplified `spawn_with_context` to bare `write_partition_from_rows(...)` (removed redundant `error!` wrapper).

5. ✅ **Three remaining callers**: Wrapped each `tx.send(...)` argument in `Ok(...)`.
   - `rust/analytics/src/lakehouse/sql_partition_spec.rs`
   - `rust/analytics/src/lakehouse/block_partition_spec.rs`
   - `rust/analytics/src/lakehouse/metadata_partition_spec.rs`

6. ✅ **`call_tree.rs`**: Improved `on_end_thread_scope` diagnostic already applied in working tree (was uncommitted). Verified: bail message includes block ID, closing scope name, and open scope name.

7. ✅ **`materialized_view.rs`**: Changed `DataFusionError::External(e.into())` to `DataFusionError::External(format!("{e:#}").into())` at the jit_update boundary.

8. ✅ `cargo fmt && cargo clippy --workspace -- -D warnings && cargo test` — all clean, 2 new tests pass:
   - `tests/call_tree_tests.rs::test_crossing_spans_returns_err`
   - `tests/write_partition_tests.rs::test_write_rows_propagates_err_from_channel`

## Files to Modify

- `rust/analytics/src/lakehouse/write_partition.rs`
- `rust/analytics/src/lakehouse/thread_spans_view.rs`
- `rust/analytics/src/lakehouse/net_spans_view.rs`
- `rust/analytics/src/lakehouse/merge.rs`
- `rust/analytics/src/lakehouse/materialized_view.rs`
- `rust/analytics/src/lakehouse/sql_partition_spec.rs`
- `rust/analytics/src/lakehouse/block_partition_spec.rs`
- `rust/analytics/src/lakehouse/metadata_partition_spec.rs`
- `rust/analytics/src/call_tree.rs`

## Trade-offs

**Query fails entirely vs. partial results per thread**: `process_spans` fails as soon as any
thread stream hits a mismatch. Per-thread isolation (log-and-skip) was considered but rejected: a
failing query with a clear error message is the right signal that the upstream instrumentation is
broken.

**Merge: orphaned object-store files on abort**: If the merge stream fails after some batches have
been sent, those Parquet files are written to object storage but `insert_partition` is skipped.
They are permanently unreferenced. This is acceptable: storage cost is bounded by the size of
whatever was written before the failure, correctness is not affected, and the alternative (phantom
merge partition) is worse — it permanently hides source data.

**Other callers that implicitly drop tx on error**: Callers that return early via `?` before
sending anything leave the channel closed without an `Err` message. The writer treats this as
"empty but committed" and calls `insert_partition`. This is the pre-existing behaviour and is
intentional for `sql_partition_spec.rs`, `block_partition_spec.rs`, and
`metadata_partition_spec.rs`.

## Testing Strategy

- Unit test in `rust/analytics/tests/`: construct a `CallTreeBuilder`, feed it crossing spans
  (BeginA → BeginB → EndA sequence), and assert that processing returns `Err`. With the improved
  diagnostic from step 6, assert the message contains both scope names and the block ID.
- Confirm that `write_rows_and_track_times` propagates `Err` from the channel without reaching
  `insert_partition`. Change the function visibility to `pub` so it can be reached from an external
  test file. Add the test to `rust/analytics/tests/write_partition_tests.rs`: construct an
  `AsyncArrowWriter<AsyncParquetWriter>` backed by `object_store::memory::InMemory` (no real
  storage or DB needed), send `Err(anyhow!("injected"))` through a
  `tokio::sync::mpsc::channel::<Result<PartitionRowSet, anyhow::Error>>`, call
  `write_rows_and_track_times`, and assert it returns `Err` with the injected message. The
  no-insert guarantee follows structurally: `write_partition_from_rows` applies `?` on
  `write_rows_and_track_times(...)`, short-circuiting before `finalize_partition_write` and
  `insert_partition` are ever reached.
- Confirm `process_spans(...)` returns a DataFusion error whose message contains the mismatch
  detail when a bad stream is included. This depends on the `{e:#}` flattening at the
  `materialized_view.rs` `scan` boundary (step 7); without it the query surfaces only the outermost
  context (`update_partition`) and this assertion fails.
