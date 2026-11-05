One row per block, the unit of telemetry upload, with the owning stream and process columns joined in (`streams.*`, `processes.*`). Global view only; query it as a plain table.

- Use it for low-level inspection: what was uploaded, when, how big, how many objects. Prefer `log_entries`, `measures` or `log_stats` for analysis.
- Filter on the joined `streams.tags` (`array_has("streams.tags", 'log')`) to restrict to one kind of data. Column names containing a dot must be double-quoted.
- `block_id` joins to the `block_id` column of `log_entries`, `measures` and `otel_spans`; `stream_id` joins to `streams`; `process_id` joins to `processes`.
- `insert_time` is the upload time; `begin_time`/`end_time` bound the events inside the block.
- Bound the query with a time range.

```sql
SELECT process_id, sum(nb_objects) AS objects, sum(payload_size) AS bytes
FROM blocks
WHERE array_has("streams.tags", 'log')
GROUP BY process_id
ORDER BY bytes DESC
LIMIT 20;
```
