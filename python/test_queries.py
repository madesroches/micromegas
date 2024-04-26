#!/usr/bin/python3
import cbor2
import datetime
import io
import pyarrow.parquet as pq
import pandas as pd
import requests
import tabulate

ANALYTICS_BASE_URL = "http://localhost:8082/analytics/"


def request(url_tail, args):
    url = ANALYTICS_BASE_URL + url_tail
    response = requests.post(
        url,
        data=cbor2.dumps(args),
    )
    if response.status_code != 200:
        raise Exception(
            "http request url={2} failed with code={0} text={1}".format(
                response.status_code, response.text, url
            )
        )
    table = pq.read_table(io.BytesIO(response.content))
    return table.to_pandas()


def req(url_tail, args={}):
    # add default args that make sense for tests but would not in general
    if "begin" not in args:
        # set a very large time span if there is not already one specified
        end = datetime.datetime.now(datetime.timezone.utc)
        begin = end - datetime.timedelta(days=10000)
        end = end + datetime.timedelta(hours=1)
        args["begin"] = begin.isoformat()
        args["end"] = end.isoformat()
    if "limit" not in args:
        args["limit"] = 1024
    return request(url_tail, args)


def test_process_list():
    df = req("query_processes")
    df = df[["process_id", "exe", "username", "start_time", "insert_time"]]
    print(tabulate.tabulate(df, headers="keys"))


def test_list_streams():
    df = req("query_streams")
    print(df)


def test_find_cpu_stream():
    df = req("query_streams", args={"tag_filter": "cpu"})
    print(df)


def test_find_cpu_blocks():
    streams_df = req("query_streams", args={"tag_filter": "cpu"})
    streams_stats = {}
    for index, row in streams_df.iterrows():
        print(row["stream_id"])
        blocks_df = req("query_blocks", args={"stream_id": row["stream_id"]})
        if len(blocks_df) == 0:
            stats = {"sum_payload": 0, "nb_events": 0}
        else:
            stats = {
                "sum_payload": blocks_df["payload_size"].sum(),
                "nb_events": blocks_df["nb_objects"].sum(),
            }
        streams_stats[row["stream_id"]] = stats
    streams_stats = pd.DataFrame(streams_stats).transpose()
    streams_stats = streams_stats[streams_stats["nb_events"] > 0]
    print(streams_stats.sort_values("nb_events", ascending=False))
