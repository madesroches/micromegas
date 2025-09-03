# Micromegas

Python analytics client for https://github.com/madesroches/micromegas/

📖 **[Complete Python API Documentation](https://madesroches.github.io/micromegas/docs/query-guide/python-api/)** - Comprehensive guide with all methods, examples, and advanced patterns

## Example usage

Query the 2 most recent log entries from the flightsql service

```python
import datetime
import micromegas

# Connect to local server
client = micromegas.connect()
sql = """
SELECT time, process_id, level, target, msg
FROM log_entries
WHERE level <= 4
AND exe LIKE '%flight%'
ORDER BY time DESC
LIMIT 2
"""

now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(minutes=2)
end = now
df = client.query(sql, begin, end)
print(df)
```

|    | time                                | process_id                           |   level | target                                 | msg                                         |
|---:|:------------------------------------|:-------------------------------------|--------:|:---------------------------------------|:--------------------------------------------|
|  0 | 2024-10-03 18:17:56.087543714+00:00 | 1db06afc-1c88-47d1-81b3-f398c5f93616 |       4 | acme_telemetry::trace_middleware       | response status=200 OK uri=/analytics/query |
|  1 | 2024-10-03 18:17:53.924037729+00:00 | 1db06afc-1c88-47d1-81b3-f398c5f93616 |       4 | micromegas_analytics::lakehouse::query | query sql=                                  |
|    |                                     |                                      |         |                                        | SELECT time, process_id, level, target, msg |
|    |                                     |                                      |         |                                        | FROM log_entries                            |
|    |                                     |                                      |         |                                        | WHERE level <= 4                            |
|    |                                     |                                      |         |                                        | AND exe LIKE '%analytics%'                  |
|    |                                     |                                      |         |                                        | ORDER BY time DESC                          |
|    |                                     |                                      |         |                                        | LIMIT 2                                     |


Query the 10 slowest top level spans in a trace within a specified time window

```python
import datetime
import micromegas

client = micromegas.connect()

# First find a stream ID
end = datetime.datetime.now(datetime.timezone.utc)
begin = end - datetime.timedelta(hours=1)
streams = client.query_streams(begin, end, limit=1)

if not streams.empty:
    stream_id = streams['stream_id'].iloc[0]
    
    sql = """
    SELECT begin, end, duration, name
    FROM view_instance('thread_spans', '{}')
    WHERE depth=1
    ORDER BY duration DESC
    LIMIT 10
    """.format(stream_id)
    
    spans = client.query(sql, begin, end)
    print(spans)
```

|    | begin                               | end                                 |   duration | name              |
|---:|:------------------------------------|:------------------------------------|-----------:|:------------------|
|  0 | 2024-10-03 18:00:59.308952900+00:00 | 2024-10-03 18:00:59.371890+00:00    |   62937100 | FEngineLoop::Tick |
|  1 | 2024-10-03 18:00:58.752476800+00:00 | 2024-10-03 18:00:58.784389+00:00    |   31912200 | FEngineLoop::Tick |
|  2 | 2024-10-03 18:00:58.701507300+00:00 | 2024-10-03 18:00:58.731479500+00:00 |   29972200 | FEngineLoop::Tick |
|  3 | 2024-10-03 18:00:59.766343100+00:00 | 2024-10-03 18:00:59.792513700+00:00 |   26170600 | FEngineLoop::Tick |
|  4 | 2024-10-03 18:00:59.282902100+00:00 | 2024-10-03 18:00:59.308952500+00:00 |   26050400 | FEngineLoop::Tick |
|  5 | 2024-10-03 18:00:59.816034500+00:00 | 2024-10-03 18:00:59.841376900+00:00 |   25342400 | FEngineLoop::Tick |
|  6 | 2024-10-03 18:00:58.897813100+00:00 | 2024-10-03 18:00:58.922769700+00:00 |   24956600 | FEngineLoop::Tick |
|  7 | 2024-10-03 18:00:59.860637+00:00    | 2024-10-03 18:00:59.885523700+00:00 |   24886700 | FEngineLoop::Tick |
|  8 | 2024-10-03 18:00:58.630051300+00:00 | 2024-10-03 18:00:58.654871500+00:00 |   24820200 | FEngineLoop::Tick |
|  9 | 2024-10-03 18:00:57.952279800+00:00 | 2024-10-03 18:00:57.977024+00:00    |   24744200 | FEngineLoop::Tick |

## Quick Start

For a complete getting started guide, see the [Python API Documentation](https://madesroches.github.io/micromegas/docs/query-guide/python-api/).

## Schema Reference

For complete schema information including all available tables, columns, and data types, see the [Schema Reference](https://madesroches.github.io/micromegas/docs/query-guide/schema-reference/).

## SQL Reference

The Micromegas analytics service is built on Apache DataFusion. For SQL syntax and functions, see the [Apache DataFusion SQL Reference](https://datafusion.apache.org/user-guide/sql/index.html).

