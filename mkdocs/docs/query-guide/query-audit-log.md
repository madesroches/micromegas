# FlightSQL Query Audit Log

The FlightSQL service emits one structured JSON log line per query, at completion, under the
dedicated `flightsql_query_audit` log target. It ties together attribution (who ran a query) and
cost (how expensive it was) in a single self-contained record, so you can answer questions like
"which clients/users are responsible for the slowest and most expensive queries?" without
correlating separate log lines and metrics.

This complements two other signals already emitted by `execute_query`:

- A free-text `info!` line at **query start**, useful for in-flight visibility.
- Untagged `imetric!` cost metrics (`query_duration_total`, `query_setup_duration`, ...), useful for
  dashboards but not filterable/groupable by client or user, since their `PropertySet` is empty.

The audit record is the structured, completion-time superset for cost attribution: it carries both
the high-cardinality attribution (full SQL, email) and the per-stage cost (durations, output rows,
bytes scanned) as one row per query.

## Querying the audit log

The audit record lands in [`log_entries`](schema-reference.md#log_entries) like any other log line,
with `target = 'flightsql_query_audit'` and the JSON payload in `msg`. Always query it with a
bounded time range plus the `target` filter — like any `log_entries` query, an unbounded scan over
this high-frequency target is expensive.

Parse `msg` with the [JSON/JSONB functions](functions-reference.md#jsonjsonb-functions)
(`jsonb_parse`, `jsonb_get`, `jsonb_as_string`, `jsonb_as_f64`, `jsonb_as_i64`, ...):

```sql
SELECT time, jsonb_parse(msg) AS j
FROM log_entries
WHERE target = 'flightsql_query_audit'
  AND time >= NOW() - INTERVAL '1 hour'
ORDER BY time DESC
LIMIT 20;
```

### Attribution and cost, grouped by client and user

```sql
WITH q AS (
  SELECT time, jsonb_parse(msg) AS j
  FROM log_entries
  WHERE target = 'flightsql_query_audit'
    AND time >= NOW() - INTERVAL '1 hour'
)
SELECT
  jsonb_as_string(jsonb_get(j, 'client')) AS client,
  jsonb_as_string(jsonb_get(j, 'email'))  AS email,
  count(*)                                            AS queries,
  sum(jsonb_as_f64(jsonb_get(j, 'total_ms')))         AS total_ms,
  approx_percentile_cont(jsonb_as_f64(jsonb_get(j, 'total_ms')), 0.95) AS p95_ms,
  sum(jsonb_as_i64(jsonb_get(j, 'bytes_scanned')))    AS bytes_scanned
FROM q
GROUP BY client, email
ORDER BY total_ms DESC;
```

### Slowest individual queries, with SQL for drill-down

```sql
WITH q AS (
  SELECT time, jsonb_parse(msg) AS j
  FROM log_entries
  WHERE target = 'flightsql_query_audit'
    AND time >= NOW() - INTERVAL '1 hour'
)
SELECT
  time,
  jsonb_as_string(jsonb_get(j, 'email')) AS email,
  jsonb_as_f64(jsonb_get(j, 'total_ms'))  AS total_ms,
  jsonb_as_i64(jsonb_get(j, 'bytes_scanned')) AS bytes_scanned,
  jsonb_as_string(jsonb_get(j, 'sql')) AS sql
FROM q
ORDER BY total_ms DESC
LIMIT 20;
```

## Fields

| Field | Type | Present | Description |
|-------|------|---------|--------------|
| `client` | string | always | Client type from the `x-client-type` metadata header (e.g. `python`, `grafana`), `unknown` if absent |
| `user` | string | always | Resolved user id |
| `email` | string | always | Resolved user email |
| `name` | string | if known | Display name from the `x-user-name` header |
| `service_account` | bool | always | `true` when the request was made by a service account delegating on behalf of a user |
| `service_account_name` | string | if delegated | Name of the delegating service account |
| `sql` | string | always | The full SQL text of the query |
| `range_begin` | string (RFC3339) | if the request specified a time range | Requested query range start |
| `range_end` | string (RFC3339) | if the request specified a time range | Requested query range end |
| `limit` | integer | if the request specified a row limit | Requested row limit |
| `context_init_ms` | float | always | Time spent creating the session context |
| `planning_ms` | float | always | Time spent building the logical plan (`ctx.sql(...)`) |
| `execution_ms` | float | always | Time spent constructing the physical plan and the response stream (not the full drain) |
| `setup_ms` | float | always | Total setup time: parsing, attribution, context creation, planning, and stream construction |
| `total_ms` | float | always | End-to-end duration, including draining the response stream to the client |
| `status` | string | always | `"ok"` or `"error"` |
| `error` | string | on error | Error message, when `status` is `"error"` |
| `output_rows` | integer | if available | Rows produced by the query's physical plan root |
| `bytes_scanned` | integer | always | Bytes read from the lakehouse's parquet reader (object-store bytes requested, which may be served from the in-process L1 cache rather than fetched from origin) |

`context_init_ms` / `planning_ms` / `execution_ms` / `setup_ms` / `total_ms` are measured with
`std::time::Instant`, independently of the raw-TSC-tick `imetric!` timings emitted elsewhere in
`execute_query` — so they don't depend on the process's TSC-frequency calibration and are reliable
on their own.

## Notes

- **One row per query, at completion.** The record can only be assembled once the response stream
  has been fully drained (or has errored), since `total_ms`, `status`, `output_rows`, and
  `bytes_scanned` only settle at that point. If a client never drains the stream, no record is
  emitted for that query.
- **`bytes_scanned` is a per-query, cache-aware signal.** It counts bytes the lakehouse parquet
  reader requested from its (possibly L1-cache-backed) object store, i.e. the bytes the query
  logically needed — not necessarily bytes fetched from origin storage. The `range_cache_origin_block_bytes`
  object-cache metric remains the process-global origin-fetch signal; the two are complementary, not
  interchangeable.
- **No fingerprint field (yet).** The raw `sql` field is enough to drill down into individual
  expensive queries; a normalized fingerprint (with literals stripped) could be added later as an
  additive field without breaking existing consumers.
