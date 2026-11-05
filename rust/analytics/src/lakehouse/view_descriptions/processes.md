One row per process that has sent telemetry. Global view only; query it as a plain table.

- Use it to resolve a process (`exe`, `computer`, `username`) and its lifetime (`start_time`, `last_update_time`) before querying `log_entries`, `measures` or the span views, then narrow the time range to that lifetime.
- `process_id` joins to `process_id` in `streams`, `blocks`, `log_entries`, `measures`, `log_stats` and to `images`, `net_spans` and `otel_spans`. `async_events` and `thread_spans` have no `process_id` column; join them through `stream_id` to `streams`, then to `processes`. `parent_process_id` links a child process to its parent.
- `properties` is JSONB; read it with `property_get(properties, 'key')` rather than as raw bytes.
- Always bound the query with the time range of the client (`--begin`).

```sql
SELECT process_id, exe, computer, start_time, last_update_time
FROM processes
WHERE exe LIKE '%server%'
ORDER BY start_time DESC
LIMIT 20;
```
