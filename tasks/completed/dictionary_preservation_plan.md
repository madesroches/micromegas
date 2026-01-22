# Dictionary Preservation for FlightSQL Queries (Web App)

## Status: COMPLETED

## Related Plans
- [Unified Metrics Query](./unified_metrics_query_plan.md) - Benefits from this optimization when fetching property JSON columns

## Scope

**Affected:**
- `analytics-web-srv` - requests dictionary preservation from FlightSQL
- `analytics-web-app` - correctly parses and renders dictionary-encoded columns

**Not affected:**
- Grafana plugin - uses its own FlightSQL client configuration
- Python API - uses Arrow Flight directly, not the Rust client
- Other Rust binaries (uri-handler, http_gateway, examples) - use `Client` directly, not `BearerFlightSQLClientFactory`

## Overview

Enable dictionary encoding preservation in FlightSQL query results from `analytics-web-srv` to reduce bandwidth for columns with repeated string values. This is particularly beneficial for JSON property columns where the same values appear across many rows.

## Background

The `jsonb_format_json` function returns `Dictionary<Int32, Utf8>` for memory efficiency - repeated JSON strings share a single dictionary entry. However, this encoding is currently lost because the FlightSQL client doesn't request it:

```
Current (without header):
flight-sql-srv ──(hydrated strings)──▶ analytics-web-srv ──(re-encode)──▶ browser

With fix (preserve_dictionary=true):
flight-sql-srv ──(dict-encoded)──▶ analytics-web-srv ──(re-encode)──▶ browser
                                          │
                                          └── encode_batch() already uses
                                              DictionaryTracker, so dictionaries
                                              are preserved automatically
```

## Implementation

### 1. Backend: Request Dictionary Preservation

**File:** `rust/public/src/client/flightsql_client_factory.rs`

Set the header in `BearerFlightSQLClientFactory::make_client()`, which is only used by `analytics-web-srv`. This ensures other clients (uri-handler, http_gateway, examples) are unaffected.

```rust
// Preserve dictionary encoding for bandwidth efficiency
client.inner_mut().set_header("preserve_dictionary", "true");
```

The `stream_query_handler` already uses `DictionaryTracker` and `IpcDataGenerator::encode` which preserve dictionary encoding in the input `RecordBatch` - no additional changes needed on the server side.

### 2. Frontend: Fix Arrow IPC Streaming

**File:** `analytics-web-app/src/lib/arrow-stream.ts`

The previous implementation created a new `RecordBatchReader` per batch by combining schema+batch bytes. This broke dictionary state since dictionaries are defined once in the schema and referenced by index in subsequent batches.

The fix uses a queue-based async generator to feed bytes to a single `RecordBatchReader`, which maintains dictionary state internally across all batches.

### 3. Frontend: Handle Dictionary-Encoded Types

**File:** `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx`

Added `unwrapDictionary()` helper to unwrap dictionary-encoded types before type checks. This ensures dictionary-encoded binary columns are correctly identified and rendered.

## Impact Analysis

| Aspect | Impact |
|--------|--------|
| Bandwidth | Reduced - repeated property values (e.g., `{"level": "high"}`) share dictionary entries instead of being repeated per-row |
| Memory (server) | Slight increase - server must track dictionary state across batches |
| Memory (browser) | Reduced - Apache Arrow JS preserves dictionary encoding in memory |
| CPU (browser) | Reduced - fewer string allocations when parsing |
| Compatibility | Required frontend fix to maintain dictionary state across batches |

## When This Matters Most

- Property values with low cardinality (few unique values, many rows)
- Long time ranges with many data points
- Properties with verbose JSON (e.g., `{"zone": "us-east-1", "tier": "production"}`)

## File Changes Summary

| File | Change |
|------|--------|
| `rust/public/src/client/flightsql_client_factory.rs` | Add `preserve_dictionary` header in `BearerFlightSQLClientFactory::make_client()` |
| `analytics-web-app/src/lib/arrow-stream.ts` | Rewrite to use queue-based async generator, maintaining dictionary state across batches |
| `analytics-web-app/src/lib/screen-renderers/TableRenderer.tsx` | Add `unwrapDictionary()` to handle dictionary-encoded binary columns |

## Verification

1. Run existing tests: `cd rust && cargo test`
2. Start services: `python3 local_test_env/ai_scripts/start_services.py`
3. Start web app backend: `cd rust && cargo run --bin analytics-web-srv`
4. Start web app frontend: `cd analytics-web-app && yarn dev`
5. Run a query with repeated string values (e.g., metrics with properties)
6. Compare response sizes before/after using browser dev tools Network tab
7. Verify Arrow JS correctly decodes dictionary-encoded columns in the web app
