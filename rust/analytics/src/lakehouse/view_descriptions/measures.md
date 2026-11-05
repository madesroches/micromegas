One row per numeric measurement (metric sample). Available as a global table and per process through `view_instance('measures', '<process_id>')`.

- High frequency. Always bound the query with a tight time range (a few hours at most) and a `LIMIT`; prefer `view_instance('measures', '<process_id>')` for a single process.
- Filter on `name` (and `unit`) first. List the names a process emits with `SELECT DISTINCT name`.
- `properties` and `process_properties` are JSONB; read them with `property_get(properties, 'key')`.
- Histogram functions (`make_histogram`, `quantile_from_histogram`, ...) aggregate `value` for percentiles.
- Joins: `process_id` to `processes`, `stream_id` to `streams`, `block_id` to `blocks`.

```sql
SELECT date_bin('1 minute', time) AS minute, avg(value) AS avg_value, max(value) AS max_value
FROM view_instance('measures', '<process_id>')
WHERE name = 'frame_time'
GROUP BY minute
ORDER BY minute
LIMIT 100;
```
