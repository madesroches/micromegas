#!/usr/bin/python3
import micromegas

import datetime
import pandas as pd
import tabulate


BASE_URL = "http://localhost:8082/"
client = micromegas.client.Client(BASE_URL)

now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(days=10000)
end = now + datetime.timedelta(hours=1)
limit = 1024


def test_list_streams():
    df = client.query_streams(begin, end, limit)
    print(df)

def test_process_streams():
    process_df = client.query_processes(begin, end, limit)
    process_df = process_df[["process_id", "exe", "start_time", "properties"]]
    for index, row in process_df.iterrows():
        streams = client.query_streams(begin, end, limit, process_id=row["process_id"])
        print(streams)
        


def test_find_cpu_stream():
    df = client.query_streams(begin, end, limit, tag_filter="cpu")
    print(df)


def get_cpu_streams_with_data():
    streams_df = client.query_streams(begin, end, limit, tag_filter="cpu")
    streams_stats = {}
    for index, row in streams_df.iterrows():
        blocks_df = client.query_blocks(begin, end, limit, row["stream_id"])
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
    df = client.query_spans(begin, end, limit, stream_id)
    print(df)
