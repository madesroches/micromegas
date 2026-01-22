# Dictionary Preservation for FlightSQL Queries (Web App)

## Status: PENDING

## Related Plans
- [Unified Metrics Query](./unified_metrics_query_plan.md) - Benefits from this optimization when fetching property JSON columns

## Scope

**Affected:** `analytics-web-srv` (web app backend only)

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

**File:** `rust/public/src/client/flightsql_client_factory.rs`

Set the header in `BearerFlightSQLClientFactory::make_client()`, which is only used by `analytics-web-srv`. This ensures other clients (uri-handler, http_gateway, examples) are unaffected.

```rust
#[async_trait]
impl FlightSQLClientFactory for BearerFlightSQLClientFactory {
    async fn make_client(&self) -> Result<Client> {
        // ... existing channel setup ...
        let mut client = Client::new(channel);

        // ... existing auth header setup ...

        // Preserve dictionary encoding for bandwidth efficiency
        client
            .inner_mut()
            .set_header("preserve_dictionary", "true");

        Ok(client)
    }
}
```

The `stream_query_handler` already uses `DictionaryTracker` and `IpcDataGenerator::encode` which preserve dictionary encoding in the input `RecordBatch` - no additional changes needed on the server side.

## Impact Analysis

| Aspect | Impact |
|--------|--------|
| Bandwidth | Reduced - repeated property values (e.g., `{"level": "high"}`) share dictionary entries instead of being repeated per-row |
| Memory (server) | Slight increase - server must track dictionary state across batches |
| Memory (browser) | Reduced - Apache Arrow JS preserves dictionary encoding in memory |
| CPU (browser) | Reduced - fewer string allocations when parsing |
| Compatibility | No client changes needed - Arrow IPC format handles dictionaries transparently |

## When This Matters Most

- Property values with low cardinality (few unique values, many rows)
- Long time ranges with many data points
- Properties with verbose JSON (e.g., `{"zone": "us-east-1", "tier": "production"}`)

## Example Savings

For 1000 rows where 90% have `{"level": "high"}`:
- Without dictionary: ~18KB (18 bytes × 1000)
- With dictionary: ~4KB (18 bytes × 1 + 4 byte indices × 1000)

This optimization benefits all queries with dictionary-encoded columns, not just the properties column.

## File Changes Summary

| File | Change |
|------|--------|
| `rust/public/src/client/flightsql_client_factory.rs` | Add `preserve_dictionary` header in `BearerFlightSQLClientFactory::make_client()` |

## Verification

1. Run existing tests: `cd rust && cargo test`
2. Start services: `python3 local_test_env/ai_scripts/start_services.py`
3. Start web app backend: `cd rust && cargo run --bin analytics-web-srv`
4. Start web app frontend: `cd analytics-web-app && yarn dev`
5. Run a query with repeated string values (e.g., metrics with properties)
6. Compare response sizes before/after using browser dev tools Network tab
7. Verify Arrow JS correctly decodes dictionary-encoded columns in the web app
