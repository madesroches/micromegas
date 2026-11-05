One row per log message. Available as a global table and per process through `view_instance('log_entries', process_id)`.

- Always bound the query with a time range. For one process, prefer `view_instance('log_entries', '<process_id>')` over `WHERE process_id = ...`, and set the time range to that process's lifetime.
- `level`: 1=Fatal, 2=Error, 3=Warn, 4=Info, 5=Debug, 6=Trace. `level <= 2` selects errors.
- `properties` (per-entry) and `process_properties` are JSONB; read them with `property_get(properties, 'key')`, or render with `jsonb_format_json(properties)`.
- `time` is the event time; `insert_time` is the upload time of its block.
- For trends and volume, use `log_stats` instead of counting rows here.
- Joins: `process_id` to `processes`, `stream_id` to `streams`, `block_id` to `blocks`.

```sql
SELECT time, level, target, msg
FROM view_instance('log_entries', '<process_id>')
WHERE level <= 3
ORDER BY time DESC
LIMIT 20;
```
