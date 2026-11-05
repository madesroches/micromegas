One row per telemetry stream. A process owns several streams (logs, metrics, one per thread for spans, and so on). Global view only; query it as a plain table.

- `tags` is a list; filter with `array_has(tags, 'log')`, `'metrics'` or `'cpu'` to pick the kind of stream. A `cpu`-tagged stream is a thread, and its `stream_id` is what `view_instance('thread_spans', stream_id)` takes.
- `properties` is JSONB; read it with `property_get(properties, 'key')`.
- `process_id` joins to `processes`; `stream_id` joins to `blocks`.
- Bound the query with a time range.

```sql
SELECT stream_id, tags, properties
FROM streams
WHERE process_id = '<process_id>' AND array_has(tags, 'cpu')
LIMIT 50;
```
