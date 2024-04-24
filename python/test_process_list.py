#!/usr/bin/python3
import requests
import pyarrow.parquet as pq
import io
import cbor2

body = cbor2.dumps({"limit": 1024})
response = requests.post(
    "http://localhost:8082/analytics/query_processes",
    data=body,
)
if response.status_code != 200:
    raise Exception(response.text)
table = pq.read_table(io.BytesIO(response.content))
print(table.to_pandas())
