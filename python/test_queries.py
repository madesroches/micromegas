#!/usr/bin/python3
import micromegas

import datetime
import pandas as pd
import tabulate


ANALYTICS_BASE_URL = "http://localhost:8082/analytics/"


def req(url_tail, args={}):
    url = ANALYTICS_BASE_URL + url_tail
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
    return micromegas.request.request(url, args)


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

def get_cpu_streams_with_data():
    streams_df = req("query_streams", args={"tag_filter": "cpu"})
    streams_stats = {}
    for index, row in streams_df.iterrows():
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
    return streams_stats

def test_find_cpu_blocks():
    streams_stats = get_cpu_streams_with_data()
    print(streams_stats.sort_values("nb_events", ascending=False))

def get_cpu_stream_with_most_events():
    streams_stats = get_cpu_streams_with_data()
    streams_stats = streams_stats.sort_values("nb_events", ascending=False)
    return streams_stats.iloc[0].name

def test_spans():
    stream_id = get_cpu_stream_with_most_events()
    df = req("query_spans", args={"stream_id": stream_id})
    print(df)
    
