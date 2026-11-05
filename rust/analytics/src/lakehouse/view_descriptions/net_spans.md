Pre-paired network bandwidth spans from a process's `net`-tagged stream. Per process only: query it through `view_instance('net_spans', '<process_id>')`; there is no global table. Partitions are built on demand when first queried.

- The horizontal axis is bits on the wire (`begin_bits` / `end_bits`), not time: a span's width is its bit contribution to its parent scope.
- `kind` is `connection`, `object`, `property` or `rpc`. A connection span is the root (`depth` 0, `parent_span_id` = -1) and wraps the objects, properties and RPCs under it. Property rows are point-in-time leaves.
- `connection_name` and `is_outgoing` are copied onto every row, so filtering by connection or direction needs no join.
- Use a tight time range and a `LIMIT`.

```sql
SELECT kind, name, sum(end_bits - begin_bits) AS bits
FROM view_instance('net_spans', '<process_id>')
WHERE is_outgoing
GROUP BY kind, name
ORDER BY bits DESC
LIMIT 20;
```
