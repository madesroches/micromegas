---
date: 2025-09-10
authors:
  - madesroches
categories:
  - Engineering
tags:
  - observability
  - reliability
  - sre
  - sql
  - telemetry
---

# Your Crash Database Can't Calculate MTBF

The most basic stability statistic is Mean Time Between Failures, and your crash reporting service can't tell you what it is. Here's why, and how unified telemetry fixes it with a single SQL query.

<!-- more -->

## The Missing Half of the Equation

MTBF requires two things: how often things fail, and how long they run between failures.

Crash reporting services — Sentry, Crashlytics, Backtrace — know the first part. They collect stack traces, count occurrences, group by signature. But they have no idea how long your software ran *successfully*. They only see failures.

To calculate MTBF, you need crash counts AND process runtimes. That means correlating data across two separate systems: your crash reporter and whatever tracks process lifecycles. Good luck joining that data when it lives in different databases, with different schemas, different time formats, and different retention policies.

## One Query in Unified Telemetry

In Micromegas, crashes are just high-severity log events. Process start times and end times are tracked as part of the same telemetry stream. Everything lives in one place, so the query is straightforward:

```sql
-- Calculate Mean Time Between Crashes (hours)
WITH crashes AS (
  SELECT process_id, count
  FROM log_stats
  WHERE level = 1  -- crashes are fatal log events
),
process_durations AS (
  SELECT
    start_time,
    arrow_cast(last_block_end_time - start_time, 'Int64') as duration_ns,
    COALESCE(crashes.count, 0) as crash_count
  FROM processes
  LEFT JOIN crashes ON processes.process_id = crashes.process_id
  WHERE exe = 'my-service.exe'
)
SELECT
  date_bin('1 day', start_time) as date,
  SUM(duration_ns / 3.6e12) / SUM(crash_count) as MTBC_hours
FROM process_durations
GROUP BY date
ORDER BY date
```

Let's walk through it:

1. **`crashes` CTE** — Counts fatal log events (level 1) per process. In Micromegas, a crash is just a log entry with the highest severity. No special crash reporting pipeline needed.

2. **`process_durations` CTE** — Joins each process with its crash count. `COALESCE` handles processes that didn't crash (count = 0). The duration is computed from process start to the last recorded telemetry block.

3. **Final SELECT** — Groups by day, sums up total runtime hours and total crashes, divides to get MTBF in hours per day.

One query. One system. Daily MTBF trend.

## The Same Query from Python

If you're building dashboards or automation, the same query works through the Python API:

```python
now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(days=30)
sql = """
WITH crashes AS (
  SELECT process_id, count
  FROM log_stats
  WHERE level = 1
),
process_durations AS (
  SELECT
    start_time,
    arrow_cast(last_block_end_time - start_time, 'Int64') as duration_ns,
    COALESCE(crashes.count, 0) as crash_count
  FROM processes
  LEFT JOIN crashes ON processes.process_id = crashes.process_id
  WHERE exe = 'my-service.exe'
)
SELECT
  date_bin('1 day', start_time) as date,
  SUM(crash_count) as crashes,
  SUM(duration_ns / 3.6e12) / SUM(crash_count) as MTBC_hours
FROM process_durations
GROUP BY date
ORDER BY date
"""
client.query(sql, begin, now)
```

Returns an Arrow table — plug it into pandas, polars, or whatever you use for analysis.

## Why This Matters

The point isn't the SQL syntax. It's that the data is in the same place.

When crashes, process lifecycles, logs, metrics, and traces all live in one queryable store, questions that used to require gluing together three systems become a single JOIN. MTBF is the simplest example. But the same principle applies to harder questions:

- **Which users are affected?** JOIN crashes with user sessions.
- **What triggers the crash?** JOIN with API call logs to find patterns in the requests preceding failures.
- **Is performance degrading before crashes?** JOIN with metric samples to spot CPU or memory trends leading up to failures.

Your crash database is a silo. Your telemetry should be a lake.

Apache DataFusion makes this practical — full SQL on cheap object storage (S3/Parquet), no per-query pricing, no cardinality traps.

[Get started with Micromegas](../../getting-started.md) or check the [query guide](../../query-guide/index.md) for more examples.
