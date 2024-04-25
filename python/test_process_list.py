#!/usr/bin/python3
import cbor2
import datetime
import io
import pyarrow.parquet as pq
import requests
import tabulate

     
def test_process_list():
    end = datetime.datetime.now(datetime.timezone.utc)
    begin = end - datetime.timedelta(days=7)
    end = end + datetime.timedelta(hours=1)
    args = {"limit": 1024, "begin": begin.isoformat(), "end": end.isoformat()}
    body = cbor2.dumps(args)
    response = requests.post(
        "http://localhost:8082/analytics/query_processes",
        data=body,
    )
    if response.status_code != 200:
        raise Exception(response.text)
    table = pq.read_table(io.BytesIO(response.content))
    print(table.schema)
    df = table.to_pandas()
    df = df[ ["process_id", "exe", "username", "start_time", "insert_time"] ]
    print(tabulate.tabulate(df, headers='keys'))
