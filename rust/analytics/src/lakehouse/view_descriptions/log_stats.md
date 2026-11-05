Log counts pre-aggregated per minute (`time_bin`), `process_id`, `level` and `target`. Global view only; query it as a plain table.

- Much cheaper than counting `log_entries` over days: use it for volume, error-rate and noisy-target analysis.
- `count` is the number of log entries in the bin; sum it, do not count rows.
- `level`: 1=Fatal, 2=Error, 3=Warn, 4=Info, 5=Debug, 6=Trace.
- `process_id` joins to `processes`. The message text is not kept; go back to `log_entries` for it.
- Bound the query with a time range.

```sql
SELECT time_bin, sum(count) AS errors
FROM log_stats
WHERE level <= 2
GROUP BY time_bin
ORDER BY time_bin
LIMIT 200;
```
