#!/usr/bin/python3
import requests
import pyarrow.parquet as pq
import io
response = requests.post("http://localhost:8082/analytics/query_processes")
if response.status_code != 200:
    raise Exception(response.text)
table = pq.read_table(io.BytesIO(response.content))
print(table.to_pandas())
