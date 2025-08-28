# env
- using the flightsql service in release
- using the python poetry env under python/micromegas

# repro steps

1. retire the all the non-global partitions

```python
import micromegas
import datetime
import pandas as pd
pd.set_option('display.max_colwidth', None)
client = micromegas.connect()

sql = """
SELECT view_set_name, view_instance_id, min(begin_insert_time) as begin, max(end_insert_time) as end
FROM list_partitions()
where view_instance_id != 'global'
group by view_set_name, view_instance_id
"""
df_instances = client.query(sql)
for _, row in df_instances.iterrows():
    client.retire_partitions(row["view_set_name"], row["view_instance_id"], row["begin"], row["end"])

```

2. generate a trace for the pocess f126ae57-7066-4f68-9378-235079bbb433 - don't bother with the sync spans

```shell
python write_perfetto.py --spans async f126ae57-7066-4f68-9378-235079bbb433
```

3. do it again - up to 10 times

```shell
python write_perfetto.py --spans async f126ae57-7066-4f68-9378-235079bbb433
```

notice that the result is different

# hypothesis

there is a race condition in the building of partitions similar to the one fixed recently - read perfetto_race_condition_fix.md
