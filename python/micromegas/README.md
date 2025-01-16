# Micromegas

Python analytics client for https://github.com/madesroches/micromegas/

## Example usage

Query the 2 most recent log entries from the flightsql service

```python
import datetime
import pandas as pd
import micromegas
import grpc

host_port = "localhost:50051"
channel_cred = grpc.local_channel_credentials()
client = micromegas.flightsql.client.FlightSQLClient(host_port, channel_cred)
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
client.query(sql, begin, end)
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
sql = """
SELECT begin, end, duration, name
FROM view_instance('thread_spans', '{stream_id}')
WHERE depth=1
ORDER BY duration DESC
LIMIT 10
;""".format(stream_id=stream_id)
client.query(sql, begin_spans, end_spans)
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

## SQL reference

The Micromegas analytics service is built on Apache DataFusion, please see [Apache DataFusion SQL Reference](https://datafusion.apache.org/user-guide/sql/index.html) for details.

## View sets

All view instances in a set have the same schema. Some view instances are global (their view_instance_id is 'global').
Global view instances are implicitly accessible to SQL queries. Non-global view instances are accessible using the table function `view_instance`.

### log_entries

```python
client.query("DESCRIBE log_entries")
```
|    | column_name   | data_type                             | is_nullable   |
|---:|:--------------|:--------------------------------------|:--------------|
|  0 | process_id    | Dictionary(Int16, Utf8)               | NO            |
|  1 | exe           | Dictionary(Int16, Utf8)               | NO            |
|  2 | username      | Dictionary(Int16, Utf8)               | NO            |
|  3 | computer      | Dictionary(Int16, Utf8)               | NO            |
|  4 | time          | Timestamp(Nanosecond, Some("+00:00")) | NO            |
|  5 | target        | Dictionary(Int16, Utf8)               | NO            |
|  6 | level         | Int32                                 | NO            |
|  7 | msg           | Utf8                                  | NO            |


#### log_entries view instances
The implicit use of the `log_entries` table corresponds to the 'global' instance, which contains the log entries of all the processes.

Except the 'global' instance, the instance_id refers to any process_id. `view_instance('log_entries', process_id)` contains that process's log. Process-specific views are materialized just-in-time and can provide much better query performance compared to the global instance.

### measures

```python
client.query("DESCRIBE measures")
```
|    | column_name   | data_type                             | is_nullable   |
|---:|:--------------|:--------------------------------------|:--------------|
|  0 | process_id    | Dictionary(Int16, Utf8)               | NO            |
|  1 | exe           | Dictionary(Int16, Utf8)               | NO            |
|  2 | username      | Dictionary(Int16, Utf8)               | NO            |
|  3 | computer      | Dictionary(Int16, Utf8)               | NO            |
|  4 | time          | Timestamp(Nanosecond, Some("+00:00")) | NO            |
|  5 | target        | Dictionary(Int16, Utf8)               | NO            |
|  6 | name          | Dictionary(Int16, Utf8)               | NO            |
|  7 | unit          | Dictionary(Int16, Utf8)               | NO            |
|  8 | value         | Float64                               | NO            |


#### measures view instances

The implicit use of the `measures` table corresponds to the 'global' instance, which contains the metrics of all the processes.

Except the 'global' instance, the instance_id refers to any process_id. `view_instance('measures', process_id)` contains that process's metrics. Process-specific views are materialized just-in-time and can provide much better query performance compared to the 'global' instance.

### thread_spans

|    | column_name   | data_type                             | is_nullable   |
|---:|:--------------|:--------------------------------------|:--------------|
|  0 | id            | Int64                                 | NO            |
|  1 | parent        | Int64                                 | NO            |
|  2 | depth         | UInt32                                | NO            |
|  3 | hash          | Uint32                                | NO            |
|  4 | begin         | Timestamp(Nanosecond, Some("+00:00")) | NO            |
|  5 | end           | Timestamp(Nanosecond, Some("+00:00")) | NO            |
|  6 | duration      | Int64                                 | NO            |
|  7 | name          | Dictionary(Int16, Utf8)               | NO            |
|  8 | target        | Dictionary(Int16, Utf8)               | NO            |
|  9 | filename      | Dictionary(Int16, Utf8)               | NO            |
|  10| line          | UInt32                                | NO            |

#### thread_spans view instances

There is no 'global' instance in the 'thread_spans' view set, there is therefore no implicit thread_spans table availble.
Users can call the table function `view_instance('thread_spans', stream_id)` to query the spans in the thread associated with the specified stream_id.


### processes

```python
client.query("DESCRIBE processes")
```
|    | column_name       | data_type                                                                                                                                                                                                                                                                                                                                        | is_nullable   |
|---:|:------------------|:-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|:--------------|
|  0 | process_id        | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
|  1 | exe               | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
|  2 | username          | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
|  3 | realname          | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
|  4 | computer          | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
|  5 | distro            | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
|  6 | cpu_brand         | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
|  7 | tsc_frequency     | Int64                                                                                                                                                                                                                                                                                                                                            | NO            |
|  8 | start_time        | Timestamp(Nanosecond, Some("+00:00"))                                                                                                                                                                                                                                                                                                            | NO            |
|  9 | start_ticks       | Int64                                                                                                                                                                                                                                                                                                                                            | NO            |
| 10 | insert_time       | Timestamp(Nanosecond, Some("+00:00"))                                                                                                                                                                                                                                                                                                            | NO            |
| 11 | parent_process_id | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
| 12 | properties        | List(Field { name: "Property", data_type: Struct([Field { name: "key", data_type: Utf8, nullable: false, dict_id: 0, dict_is_ordered: false, metadata: {} }, Field { name: "value", data_type: Utf8, nullable: false, dict_id: 0, dict_is_ordered: false, metadata: {} }]), nullable: false, dict_id: 0, dict_is_ordered: false, metadata: {} }) | NO            |


There is only one instance in this view set and it is implicitly available.


### streams

```python
client.query("DESCRIBE streams")
```
|    | column_name           | data_type                                                                                                                                                                                                                                                                                                                                        | is_nullable   |
|---:|:----------------------|:-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|:--------------|
|  0 | stream_id             | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
|  1 | process_id            | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
|  2 | dependencies_metadata | Binary                                                                                                                                                                                                                                                                                                                                           | NO            |
|  3 | objects_metadata      | Binary                                                                                                                                                                                                                                                                                                                                           | NO            |
|  4 | tags                  | List(Field { name: "tag", data_type: Utf8, nullable: false, dict_id: 0, dict_is_ordered: false, metadata: {} })                                                                                                                                                                                                                                  | YES           |
|  5 | properties            | List(Field { name: "Property", data_type: Struct([Field { name: "key", data_type: Utf8, nullable: false, dict_id: 0, dict_is_ordered: false, metadata: {} }, Field { name: "value", data_type: Utf8, nullable: false, dict_id: 0, dict_is_ordered: false, metadata: {} }]), nullable: false, dict_id: 0, dict_is_ordered: false, metadata: {} }) | NO            |
|  6 | insert_time           | Timestamp(Nanosecond, Some("+00:00"))                                                                                                                                                                                                                                                                                                            | NO            |

There is only one instance in this view set and it is implicitly available.

### blocks


```python
client.query("DESCRIBE blocks")
```

|    | column_name                   | data_type                                                                                                                                                                                                                                                                                                                                        | is_nullable   |
|---:|:------------------------------|:-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|:--------------|
|  0 | block_id                      | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
|  1 | stream_id                     | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
|  2 | process_id                    | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
|  3 | begin_time                    | Timestamp(Nanosecond, Some("+00:00"))                                                                                                                                                                                                                                                                                                            | NO            |
|  4 | begin_ticks                   | Int64                                                                                                                                                                                                                                                                                                                                            | NO            |
|  5 | end_time                      | Timestamp(Nanosecond, Some("+00:00"))                                                                                                                                                                                                                                                                                                            | NO            |
|  6 | end_ticks                     | Int64                                                                                                                                                                                                                                                                                                                                            | NO            |
|  7 | nb_objects                    | Int32                                                                                                                                                                                                                                                                                                                                            | NO            |
|  8 | object_offset                 | Int64                                                                                                                                                                                                                                                                                                                                            | NO            |
|  9 | payload_size                  | Int64                                                                                                                                                                                                                                                                                                                                            | NO            |
| 10 | insert_time                   | Timestamp(Nanosecond, Some("+00:00"))                                                                                                                                                                                                                                                                                                            | NO            |
| 11 | streams.dependencies_metadata | Binary                                                                                                                                                                                                                                                                                                                                           | NO            |
| 12 | streams.objects_metadata      | Binary                                                                                                                                                                                                                                                                                                                                           | NO            |
| 13 | streams.tags                  | List(Field { name: "tag", data_type: Utf8, nullable: false, dict_id: 0, dict_is_ordered: false, metadata: {} })                                                                                                                                                                                                                                  | YES           |
| 14 | streams.properties            | List(Field { name: "Property", data_type: Struct([Field { name: "key", data_type: Utf8, nullable: false, dict_id: 0, dict_is_ordered: false, metadata: {} }, Field { name: "value", data_type: Utf8, nullable: false, dict_id: 0, dict_is_ordered: false, metadata: {} }]), nullable: false, dict_id: 0, dict_is_ordered: false, metadata: {} }) | NO            |
| 15 | processes.start_time          | Timestamp(Nanosecond, Some("+00:00"))                                                                                                                                                                                                                                                                                                            | NO            |
| 16 | processes.start_ticks         | Int64                                                                                                                                                                                                                                                                                                                                            | NO            |
| 17 | processes.tsc_frequency       | Int64                                                                                                                                                                                                                                                                                                                                            | NO            |
| 18 | processes.exe                 | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
| 19 | processes.username            | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
| 20 | processes.realname            | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
| 21 | processes.computer            | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
| 22 | processes.distro              | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
| 23 | processes.cpu_brand           | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
| 24 | processes.insert_time         | Timestamp(Nanosecond, Some("+00:00"))                                                                                                                                                                                                                                                                                                            | NO            |
| 25 | processes.parent_process_id   | Utf8                                                                                                                                                                                                                                                                                                                                             | NO            |
| 26 | processes.properties          | List(Field { name: "Property", data_type: Struct([Field { name: "key", data_type: Utf8, nullable: false, dict_id: 0, dict_is_ordered: false, metadata: {} }, Field { name: "value", data_type: Utf8, nullable: false, dict_id: 0, dict_is_ordered: false, metadata: {} }]), nullable: false, dict_id: 0, dict_is_ordered: false, metadata: {} }) | NO            |

There is only one instance in this view set and it is implicitly available.
