#!/usr/bin/python3
import tabulate
from .test_utils import *


def test_list_streams():
    df = client.query_streams(begin, end, limit)
    print(df)

def test_process_streams():
    sql = """
    SELECT processes.process_id, stream_id, cpu_brand
    FROM   streams, processes
    WHERE streams.process_id = processes.process_id
    ORDER BY streams.insert_time
    LIMIT 10;
    """
    df = client.query(sql)
    print("\n", df)
        
def test_find_cpu_stream():
    df = client.query_streams(begin, end, limit, tag_filter="cpu")
    print(df)

def test_spans():
    blocks = client.query("""
    SELECT stream_id, "streams.tags", nb_objects
    FROM blocks
    WHERE array_has( "streams.tags", 'cpu' )
    ORDER BY nb_objects DESC
    LIMIT 1;
    """)
    stream_id = blocks.iloc[0]["stream_id"]
    df = client.query_spans(begin, end, limit, stream_id)
    print(df)
