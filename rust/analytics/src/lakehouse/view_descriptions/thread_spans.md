One row per completed synchronous span (a begin/end pair) on a thread, plus one synthetic thread-root row per contiguous block chain in each partition. Per stream only: query it through `view_instance('thread_spans', '<stream_id>')`, where the stream is a thread's `cpu`-tagged stream, or for a whole process through `process_spans('<process_id>', 'thread')`. There is no global table.

- Spans are extremely dense. Always use a tight time range and a `LIMIT`.
- A scan is ordered by `begin`, so `ORDER BY begin` is free.
- `id` and `parent` build the call tree; `depth` is the nesting level (real top-level spans are at depth 1); `duration` is in nanoseconds.
- `properties` holds the end-event properties as JSONB; read it with `property_get(properties, 'key')`.
- Each root row has `id = -1`, `depth = 0`, spans only its chain of blocks (a partition with gaps has several) and is named after the thread; filter it out with `WHERE depth > 0` when ranking spans.
- Find the thread streams of a process in `streams` (`array_has(tags, 'cpu')`).

```sql
SELECT name, target, duration, depth
FROM view_instance('thread_spans', '<stream_id>')
WHERE depth > 0
ORDER BY duration DESC
LIMIT 20;
```
