# OTLP Zero-Timestamp Backfill Plan

Addresses [issue #1123](https://github.com/madesroches/micromegas/issues/1123).

## Overview

OTLP log records where both `time_unix_nano` and `observed_time_unix_nano` are zero are
silently dropped at analytics time, producing empty `log_entries` results even though
ingestion returns 200 OK. The fix is to backfill `observed_time_unix_nano` at ingestion
time — before encoding the proto payload — which is exactly what the OTLP spec requires
of the collecting system. A companion change adds `debug!` logging at both the ingestion
and analytics layers so operators can detect data loss.

## Current State

### Ingestion (`rust/otel-ingestion/src/block.rs`)

`split_logs` (line 248) calls `logs_bounds` to derive `begin_time`/`end_time` for the
block envelope. `logs_bounds` (line 41) detects the all-zero case and returns
`Some((0, 0, count))`, which causes `build_prepared_block` (line 188) to substitute
`Utc::now()` for the envelope timestamps. **However**, the `ResourceLogs` proto is then
encoded as-is (line 259) with the zero timestamps still in place.

### Analytics (`rust/analytics/src/lakehouse/otel/logs_block_processor.rs`)

`OtelLogsBlockProcessor::process()` (lines 102–112) reads the stored proto and skips any
`LogRecord` where both fields are zero, incrementing `nb_dropped_no_timestamp`. A single
aggregated `debug!` fires at line 199 if any records were dropped. When the entire block
consists of zero-timestamp records, `nb_appended == 0` and `process()` returns `Ok(None)`
— the partition gets no rows and `log_entries` returns nothing.

## Design

### Backfill in `split_logs`

Mutate each `ResourceLogs` before encoding: iterate over all `scope_logs` / `log_records`
and set `record.observed_time_unix_nano = now_nanos` wherever both fields are zero.
Capture `now` once per `ResourceLogs` (not per record) so all records in the same batch
share the same observed timestamp and the block envelope is consistent with the payload.

After the backfill `logs_bounds` returns a real non-zero range, so the envelope, the
stored proto, and the analytics processor all see the same wall-clock time.

Add a `debug!` log at ingestion when records are backfilled (count + block context),
mirroring the existing pattern at the analytics layer.

### Logging when dropping data

**Ingestion side** (new): emit `debug!` listing how many records had timestamps backfilled,
per `ResourceLogs`. This surfaces in the telemetry stream so operators can tell that
a sender is omitting timestamps without needing to dig into analytics logs.

**Analytics side** (existing, no change needed): the `debug!` at line 199 of
`logs_block_processor.rs` already fires when records are dropped. With the fix applied,
this should never trigger for OTLP blocks ingested after the fix, but the guard stays as
a defensive safety net.

## Implementation Steps

1. **`rust/otel-ingestion/src/block.rs` — backfill in `split_logs`**
   - Change `for rl in req.resource_logs` to `for mut rl in req.resource_logs`.
   - Before calling `logs_bounds`, iterate `rl.scope_logs` mutably, backfilling
     `observed_time_unix_nano` on each record where both fields are `0`.
   - Capture `now_nanos` once before the inner loop (a `u64` from
     `Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64`).
   - Accumulate a backfill count; emit `debug!` if `> 0`.

2. **`rust/otel-ingestion/tests/fixtures.rs` — add `log_record_no_timestamp` helper**
   - A variant of `log_record` where both `time_unix_nano` and `observed_time_unix_nano`
     are `0`. Keeps test call-sites explicit and readable.

3. **`rust/otel-ingestion/tests/split_tests.rs` — add regression tests**
   - `logs_split_backfills_observed_time_when_both_timestamps_zero`: build a request with
     `log_record_no_timestamp` records, call `split_logs`, assert `begin_time` and
     `end_time` are after a sentinel date (i.e., not epoch), then re-decode
     `block.payload.objects` as `ResourceLogs` and assert every record's
     `observed_time_unix_nano != 0`.
   - `logs_split_preserves_existing_observed_time`: build a request where records already
     have `observed_time_unix_nano != 0` and `time_unix_nano == 0`; assert the value is
     unchanged after `split_logs`.
   - `logs_split_mixed_timestamps_all_survive`: a block with some zero-timestamp records
     and some non-zero; assert all records survive, `begin_time` equals the minimum
     original non-zero timestamp, and `end_time` is after a sentinel date (e.g.,
     2024-01-01) rather than matching the exact max of the original non-zero range
     (because zero-timestamp records are backfilled to `now_nanos`, which is greater than
     any historical fixture timestamp).

## Files to Modify

| File | Change |
|------|--------|
| `rust/otel-ingestion/src/block.rs` | Backfill `observed_time_unix_nano`, add `debug!` |
| `rust/otel-ingestion/tests/fixtures.rs` | Add `log_record_no_timestamp` helper |
| `rust/otel-ingestion/tests/split_tests.rs` | Add 3 regression tests |

Analytics files (`logs_block_processor.rs`) require no changes — the existing guard and
debug log stay as-is.

## Trade-offs

**Alternative: fix in the analytics processor** — use `src_block.block.begin_time` as a
fallback when both fields are zero. Rejected: the block envelope time is a coarse
per-block value shared by all records; it would not give individual records an accurate
`observed_time_unix_nano`. It would also leave the stored proto non-conformant for any
future consumer.

**Alternative: fix in both layers** — backfill at ingestion *and* relax the analytics
guard to use `insert_time` as last-resort. Rejected as over-engineering; the single fix
at ingestion is sufficient and keeps the analytics layer simple.

**Idempotency impact of pre-encode mutation** — `block_id_from_payload` (called in
`identity.rs` line 143 and `block.rs` line 202) computes a UUID-v5 over
`rl.encode_to_vec()`. Because the backfill mutates `observed_time_unix_nano` to
`now_nanos` *before* encoding, two retries of a byte-identical zero-timestamp payload
arrive at a different encoded byte sequence → different `block_id` → both pass the
`ON CONFLICT (block_id) DO NOTHING` guard in `web_ingestion_service.rs` line 136,
creating duplicate blocks and ultimately duplicate `log_entries` rows.
Mitigation: compute `block_id` from the **pre-mutation** bytes by calling
`block_id_from_payload` before the backfill loop, then passing the pre-computed ID
through to `build_prepared_block` instead of letting it be re-derived from the mutated
proto. This preserves retry idempotency without changing the stored payload or the
analytics output. The implementation step for `block.rs` should be updated to reflect
this ordering (capture `block_id` → backfill → encode).

## Testing Strategy

- Unit tests in `split_tests.rs` verify the backfill logic without touching Postgres or
  object storage (pure proto manipulation).
- Run `cargo test -p micromegas-otel-ingestion` after the change.
- Manual smoke test: send an OTLP/JSON payload with `timeUnixNano` omitted (e.g. using
  `curl`) to a local ingestion service and confirm `log_entries` returns rows.

## Open Questions

None — the fix is fully specified by the OTLP spec and the issue.
