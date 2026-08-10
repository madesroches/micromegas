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

### Most memory-hungry individual queries, with SQL for drill-down

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
  jsonb_as_i64(jsonb_get(j, 'peak_memory_bytes')) AS peak_memory_bytes,
  jsonb_as_i64(jsonb_get(j, 'spilled_bytes')) AS spilled_bytes,
  jsonb_as_string(jsonb_get(j, 'sql')) AS sql
FROM q
ORDER BY peak_memory_bytes DESC
LIMIT 20;
```

### Failed queries grouped by `error_class`

`error_class` distinguishes "the caller's SQL/input was bad" (`"user"`), "the query exceeded a
resource budget" (`"resource"`), and "a genuine server-side failure" (`"internal"`) -- useful for
telling how much of your error rate is actually actionable by the caller versus a real service
problem.

```sql
WITH q AS (
  SELECT time, jsonb_parse(msg) AS j
  FROM log_entries
  WHERE target = 'flightsql_query_audit'
    AND time >= NOW() - INTERVAL '1 hour'
)
SELECT
  jsonb_as_string(jsonb_get(j, 'error_class')) AS error_class,
  count(*) AS failures
FROM q
WHERE jsonb_as_string(jsonb_get(j, 'status')) = 'error'
GROUP BY error_class
ORDER BY failures DESC;
```

## Fields

| Field | Type | Present | Description |
|-------|------|---------|--------------|
| `query_id` | string (UUID) | always | Unique id minted at the start of the request; also embedded in the client-facing error message and the server-side log line for the same failure, so the three can be correlated by grepping this id |
| `client` | string | always | Client type from the `x-client-type` metadata header (e.g. `python`, `grafana`), `unknown` if absent |
| `agent` | string | always | Who is driving the client, from the `x-client-agent` metadata header (e.g. `claude-code`, `none`), `unknown` if absent |
| `entrypoint` | string | always | How the client was invoked, from the `x-client-entrypoint` metadata header (e.g. `script`, `jupyter`, `repl`, `cli-query`), `unknown` if absent |
| `session` | string | if the caller sent `x-client-session` | Opaque id correlating every query issued through one client instance/session |
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
| `status` | string | always | `"ok"`, `"error"`, or `"incomplete"` (stream abandoned mid-drain, e.g. client disconnect or cancellation) |
| `error` | string | on error | Error message, when `status` is `"error"` |
| `error_class` | string | on error | `"user"` (bad SQL/input), `"resource"` (query exceeded a resource budget), or `"internal"` (a genuine server-side failure), derived from the gRPC status code the query failed with |
| `output_rows` | integer | if available | Rows produced by the query's physical plan root |
| `bytes_scanned` | integer | always | Bytes read from the lakehouse's parquet reader (object-store bytes requested, which may be served from the in-process L1 cache rather than fetched from origin) |
| `peak_memory_bytes` | integer | always | Peak tracked DataFusion reservation for this query alone |
| `spilled_bytes` | integer | always | Total bytes spilled to disk by this query's plan; nonzero only once the query actually spills — the exceptional safety-valve path, not the common case |
| `spill_count` | integer | always | Number of spill events for this query's plan; nonzero only once the query actually spills |

`context_init_ms` / `planning_ms` / `execution_ms` / `setup_ms` / `total_ms` are measured with
`std::time::Instant`, independently of the raw-TSC-tick `imetric!` timings emitted elsewhere in
`execute_query` — so they don't depend on the process's TSC-frequency calibration and are reliable
on their own. On a record with `status = "error"` emitted during setup, stage fields for stages
that were never reached read `0.0`; `total_ms` still covers the full request.

## Notes

- **`agent`/`entrypoint`/`session` distinguish three states, not two.** `unknown` means the
  client didn't report that header at all (e.g. Grafana, or any client older than this feature).
  `none` (for `agent`) or `script` (for `entrypoint`) means the Python client actively resolved
  the value and found nothing distinctive — a real, reported answer, not an absence. A detected
  value (`claude-code`, `jupyter`, `repl`, `cli-query`, ...) means the client found and reported a
  specific signal. Don't conflate `unknown` with `none`/`script` when grouping or filtering.
- **`agent` measures environment provenance, not SQL authorship.** It reflects whether the
  client process ran inside a known agent harness's environment (detected from that harness's
  marker environment variable), not whether an LLM actually wrote the SQL text. Environment
  variables are inherited by child processes, so a human running `micromegas-query` from a shell
  nested inside an agent session (e.g. a terminal opened from within Claude Code) is labelled
  with that agent too, even though a person typed the query.
- **One row per query, at completion.** The record can only be assembled once the response stream
  has been fully drained (or has errored), since `total_ms`, `status`, `output_rows`, and
  `bytes_scanned` only settle at that point. If a client abandons the stream mid-drain (disconnect
  or cancellation), a record is still emitted with `status = "incomplete"`; its cost fields reflect
  the work done up to that point.
- **`bytes_scanned` is a per-query, cache-aware signal.** It counts bytes the lakehouse parquet
  reader requested from its (possibly L1-cache-backed) object store, i.e. the bytes the query
  logically needed — not necessarily bytes fetched from origin storage. The `range_cache_origin_block_bytes`
  object-cache metric remains the process-global origin-fetch signal; the two are complementary, not
  interchangeable.
- **No fingerprint field (yet).** The raw `sql` field is enough to drill down into individual
  expensive queries; a normalized fingerprint (with literals stripped) could be added later as an
  additive field without breaking existing consumers.
- **`peak_memory_bytes` is a per-query lower bound on process cost, not the full picture.** It's the
  peak of *tracked* DataFusion reservation — the same mechanism DataFusion's own memory-limit
  enforcement uses — but it doesn't count in-flight `RecordBatch`es and parquet decode buffers
  (DataFusion documents this as deliberate), the L1 byte cache or micromegas' metadata cache
  (separately accounted), or `AsyncArrowWriter` row-group buffers used by JIT materialization
  inside a query (bounded, since row groups are capped at 128K rows). It is a monotonic
  high-water mark, so it stays valid on `error` and `incomplete` records too — even a record whose
  query failed or was abandoned reports the real peak reached before that point. It is the signal
  to use when judging whether the deployed `MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB` is set
  correctly and which queries are pushing against it; `spilled_bytes`/`spill_count` are the alarm
  for when a query actually leans on the disk-spill safety valve rather than merely coming close.
- **`peak_memory_bytes` and `spilled_bytes`/`spill_count` don't share the same scope.** The peak
  naturally includes nested session contexts built during execution (Perfetto trace queries, JIT
  materialization), since it comes from the query's own memory-pool instance. The spill counters
  instead come from summing the *outer* physical plan tree, so a nested session context built
  inside a leaf node is opaque to that sum — a query can legitimately show `peak_memory_bytes > 0`
  with `spilled_bytes == 0` even when nested work spilled. Also, a query that runs
  `materialize_partitions()`/`regenerate_partitions()` reports the merge's peak against the calling
  query, understated by the row-group-buffer caveat above.
