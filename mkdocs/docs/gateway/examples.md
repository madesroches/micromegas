# Gateway Examples

Practical examples and integration patterns for the HTTP Gateway.

## Basic Usage

### Simple Query

Basic query without authentication:

```bash
curl -X POST http://localhost:3000/gateway/query \
  -H "Content-Type: application/json" \
  -d '{
    "sql": "SELECT * FROM processes LIMIT 10"
  }'
```

Response:

```json
[
  {
    "process_id": "01JGXZ...",
    "exe": "my-app",
    "username": "alice",
    "start_time": "2025-01-15T10:30:00Z"
  },
  ...
]
```

### Query with Time Range

**⭐ Recommended:** Use out-of-band time range filtering for efficient partition elimination:

```bash
curl -X POST http://localhost:3000/gateway/query \
  -H "Content-Type: application/json" \
  -d '{
    "sql": "SELECT * FROM log_entries ORDER BY time DESC LIMIT 100",
    "time_range_begin": "2024-01-01T00:00:00Z",
    "time_range_end": "2024-01-01T23:59:59Z"
  }'
```

The time range parameters enable partition elimination before query execution, providing significant performance improvements over SQL time filters.

**Time Range Format:**
- RFC3339 timestamps with timezone (e.g., `"2024-01-01T00:00:00Z"`)
- Both `time_range_begin` and `time_range_end` must be provided together
- Begin must be before end

**Alternative (less efficient):** Using SQL time filters:

```bash
curl -X POST http://localhost:3000/gateway/query \
  -H "Content-Type: application/json" \
  -d '{
    "sql": "SELECT * FROM log_entries WHERE time > NOW() - INTERVAL '\''1 hour'\'' LIMIT 100"
  }'
```

Note: SQL time filters scan all partitions, while API time ranges enable partition elimination.

### Query with Authentication

Query with OIDC bearer token:

```bash
curl -X POST http://localhost:3000/gateway/query \
  -H "Authorization: Bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9..." \
  -H "Content-Type: application/json" \
  -d '{
    "sql": "SELECT * FROM streams WHERE stream_type = '\''metrics'\''"
  }'
```
