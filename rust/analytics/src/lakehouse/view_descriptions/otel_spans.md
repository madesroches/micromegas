OpenTelemetry spans from OTLP-ingested traces. Per process only: query it through `view_instance('otel_spans', '<process_id>')`; there is no global table. Searching a trace across services means a UNION over each participating process.

- `trace_id` (16 bytes) and `span_id` / `parent_span_id` (8 bytes) are binary. Render with `encode(trace_id, 'hex')` and filter with `trace_id = decode('<hex>', 'hex')`. `parent_span_id` is NULL on root spans.
- `kind` is `INTERNAL`, `SERVER`, `CLIENT`, `PRODUCER`, `CONSUMER` or `UNSPECIFIED`; `status` is `OK`, `ERROR` or `UNSET`.
- `duration` is in nanoseconds.
- `properties` holds span attributes plus scope information under `otel.scope.*` keys (JSONB); read it with `property_get(properties, 'key')`. `events` and `links` are JSONB arrays.
- Use a tight time range and a `LIMIT`.

```sql
SELECT name, kind, status, duration, encode(span_id, 'hex') AS span
FROM view_instance('otel_spans', '<process_id>')
WHERE trace_id = decode('0123456789abcdef0123456789abcdef', 'hex')
ORDER BY start_time;
```
