#!/usr/bin/python3
import tabulate
from .test_utils import *


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

def test_find_cpu_blocks():
    streams_stats = get_tagged_streams_with_data("cpu")
    print(streams_stats.sort_values("nb_events", ascending=False))


def test_spans():
    stream_id = get_tagged_stream_with_most_events("cpu")
    df = client.query_spans(begin, end, limit, stream_id)
    print(df)
