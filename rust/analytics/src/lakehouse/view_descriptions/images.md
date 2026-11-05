One row per image (such as a screenshot) captured with `send_image()`. Per process only: query it through `view_instance('images', '<process_id>')`; there is no global table.

- `data` holds the raw bytes in `format` (for example `png`); select it only when you need the image, since it is large. Use `payload_size` and `name` to browse.
- Bound the query with the process's time range.
- `process_id` joins to `processes`, `stream_id` to `streams`, `block_id` to `blocks`.

```sql
SELECT time, name, format, payload_size
FROM view_instance('images', '<process_id>')
ORDER BY time DESC
LIMIT 20;
```
