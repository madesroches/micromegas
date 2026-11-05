One row per async span event: a `begin` or `end` marker (`event_type`) for an async span. Per process only: query it through `view_instance('async_events', '<process_id>')` (`process_spans('<process_id>', 'async')` is different: it returns already-paired spans with `thread_spans`-shaped columns plus `stream_id` and `thread_name`, not `event_type`/`span_id`/`time`). There is no global table.

- Always use a tight time range and a `LIMIT`; async events are dense.
- `span_id` pairs a `begin` row with its `end` row; `parent_span_id` and `depth` give the hierarchy. Duration is the difference between the two `time` values (join or pivot on `span_id`).
- `properties` holds the end-event properties as JSONB (empty on `begin` rows); read it with `property_get(properties, 'key')`.
- `stream_id` joins to `streams`, `block_id` to `blocks`.

```sql
SELECT b.name, e.time - b.time AS duration
FROM view_instance('async_events', '<process_id>') b
JOIN view_instance('async_events', '<process_id>') e
  ON b.span_id = e.span_id AND e.event_type = 'end'
WHERE b.event_type = 'begin'
ORDER BY duration DESC
LIMIT 20;
```
