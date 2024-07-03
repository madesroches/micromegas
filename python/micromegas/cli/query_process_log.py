import argparse
import datetime
import importlib
import micromegas
import os
import sys
from tabulate import tabulate

def fetch_last_n(limit, client, df_blocks, stream_id):
    nb_blocks = df_blocks.shape[0]
    last_block = df_blocks.iloc[nb_blocks-1]
    begin = last_block["begin_time"]
    end = last_block["end_time"]
    nb_objects = last_block["nb_objects"]
    for index in range(nb_blocks-2, -1, -1):
        if nb_objects >= limit:
            break
        block = df_blocks.iloc[index]
        nb_objects += block["nb_objects"]
        begin = block["begin_time"]
    df_log = client.query_log_entries(begin, end, int(nb_objects), stream_id=stream_id)
    return df_log.tail(limit).copy().reset_index()

def fetch_first_n(limit, client, process, df_blocks, stream_id):
    last_end = df_blocks["end_time"].max()
    return client.query_log_entries(process["start_time"], last_end, limit=limit, stream_id=stream_id)

def main():
    parser = argparse.ArgumentParser(
        prog="query_process_log",
        description="List log entries associated with a specific process",
    )
    parser.add_argument("--first")
    parser.add_argument("--last")
    parser.add_argument("process_id")
    args = parser.parse_args()

    micromegas_module_name = os.environ.get(
        "MICROMEGAS_PYTHON_MODULE_WRAPPER", "micromegas"
    )
    micromegas_module = importlib.import_module(micromegas_module_name)
    client = micromegas_module.connect()
    df_process = client.find_process(args.process_id)
    if df_process.empty:
        print("process not found")
        sys.exit(1)
    assert df_process.shape[0] == 1
    process = df_process.iloc[0]
    process_id = process["process_id"]
    df_streams = client.query_streams(
        begin=None, end=None, limit=1024, tag_filter="log", process_id=process_id
    )
    if df_streams.empty:
        print("log stream not found")
        sys.exit(1)
    assert df_streams.shape[0] == 1
    stream = df_streams.iloc[0]
    stream_id = stream["stream_id"]

    df_blocks = client.query_blocks(
        begin=None, end=None, limit=1024, stream_id=stream_id
    )
    if df_blocks.empty:
        print("no log entries")
        sys.exit(0)

    if args.last is not None:
        assert args.first is None
        df_log = fetch_last_n(int(args.last), client, df_blocks, stream_id)
    else:
        limit = args.first or 1024 * 1024
        df_log = fetch_first_n(int(limit), client, process, df_blocks, stream_id)

    print(tabulate(df_log, headers="keys"))


if __name__ == "__main__":
    main()
