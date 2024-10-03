# Micromegas

Python analytics client for https://github.com/madesroches/micromegas/

## Example usage

Printing the most recent 10 log entries limiting the search to the last 2 minutes

```python
import datetime
import pandas as pd
import micromegas

BASE_URL = "http://localhost:8082/"
client = micromegas.client.Client(BASE_URL)
sql = """
SELECT time, process_id, level, target, msg
FROM log_entries
WHERE level <= 4
ORDER BY time DESC
LIMIT 10
;"""

now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(minutes=2)
end = now
df = client.query(sql, begin, end)
print(df)
```

## Implicitly available views and their schema

```python
client.query( "DESCRIBE log_entries")
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


```python
client.query( "DESCRIBE measures")
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

```python
client.query( "DESCRIBE processes")
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


```python
client.query( "DESCRIBE streams" )
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


```python
client.query( "DESCRIBE blocks" )
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
