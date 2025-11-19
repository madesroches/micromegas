# HTTP Gateway Time Range Filtering

## Overview

The HTTP gateway now supports out-of-band time range filtering, similar to the FlightSQL service and Python API. This allows clients to filter query results by time without modifying the SQL query itself.

## Implementation Details

### Changes Made

1. **Updated `QueryRequest` struct** (`rust/public/src/servers/http_gateway.rs:135-146`)
   - Added optional `time_range_begin` field (RFC3339 timestamp string)
   - Added optional `time_range_end` field (RFC3339 timestamp string)

2. **Added time range parsing logic** (`rust/public/src/servers/http_gateway.rs:234-271`)
   - Validates that both begin and end are provided together
   - Parses RFC3339 timestamps into `DateTime<Utc>`
   - Validates that begin comes before end
   - Returns appropriate error messages for invalid input

3. **Updated query execution** (`rust/public/src/servers/http_gateway.rs:330`)
   - Passes `time_range` parameter to `client.query()` instead of hardcoded `None`
   - FlightSQL client sets `query_range_begin` and `query_range_end` headers internally

## API Usage

### Request Format

```json
{
  "sql": "SELECT * FROM processes",
  "time_range_begin": "2024-01-01T00:00:00Z",
  "time_range_end": "2024-01-02T00:00:00Z"
}
```

### Valid Timestamp Formats

Timestamps must be in RFC3339 format with timezone:
- `2024-01-01T00:00:00Z` (UTC)
- `2024-01-01T00:00:00+00:00` (UTC with offset)
- `2024-01-01T12:30:45.123456789Z` (with nanoseconds)
- `2024-01-01T08:00:00-05:00` (EST timezone)

### Validation Rules

1. **Both or neither**: You must provide both `time_range_begin` and `time_range_end`, or omit both
2. **Valid order**: `time_range_begin` must be before `time_range_end`
3. **Valid format**: Timestamps must be valid RFC3339 strings

### Error Responses

**Missing one timestamp:**
```
400 Bad Request
"time_range_end must be provided when time_range_begin is specified"
```

**Invalid order:**
```
400 Bad Request
"time_range_begin must be before time_range_end"
```

**Invalid format:**
```
400 Bad Request
"Invalid time_range_begin format (expected RFC3339): ..."
```

## Testing

A test script is provided at `test_gateway_time_range.py` that demonstrates:
1. Query without time range (default behavior)
2. Query with valid time range
3. Query with invalid time range (begin > end)
4. Query with partial time range (only begin)
5. Query with malformed timestamps

Run the tests with:
```bash
python3 test_gateway_time_range.py
```

## Consistency with Other APIs

This implementation follows the same pattern used by:

1. **FlightSQL Client** (`rust/public/src/client/flightsql_client.rs:39-48`)
   - Sets `query_range_begin` and `query_range_end` headers

2. **FlightSQL Service** (`rust/public/src/servers/flight_sql_service_impl.rs:187-200`)
   - Reads these headers and creates `TimeRange` for query filtering

3. **Python API** (`python/micromegas/cli/write_perfetto.py:40-46`)
   - Accepts `--begin` and `--end` command-line arguments in ISO format

## Example Curl Commands

**Query without time range:**
```bash
curl -X POST http://localhost:3000/gateway/query \
  -H "Content-Type: application/json" \
  -d '{"sql": "SELECT COUNT(*) FROM processes"}'
```

**Query with time range:**
```bash
curl -X POST http://localhost:3000/gateway/query \
  -H "Content-Type: application/json" \
  -d '{
    "sql": "SELECT COUNT(*) FROM processes",
    "time_range_begin": "2024-01-01T00:00:00Z",
    "time_range_end": "2024-01-02T00:00:00Z"
  }'
```

## Benefits

1. **Out-of-band filtering**: Time range filters don't pollute SQL queries
2. **Consistent API**: Matches FlightSQL and Python client behavior
3. **Type safety**: Rust type system validates time ranges at compile time
4. **Better errors**: Clear validation messages for common mistakes
5. **Flexibility**: Clients can dynamically adjust time ranges without SQL changes
