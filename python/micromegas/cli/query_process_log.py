import argparse
import datetime
import importlib
import micromegas
import os
import sys
from tabulate import tabulate


def main():
    parser = argparse.ArgumentParser(
        prog="query_process_log",
        description="List log entries associated with a specific process",
    )
    parser.add_argument("--first")
    parser.add_argument("--last")
    parser.add_argument("--target")
    parser.add_argument("--maxlevel", default=6)
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

    last_end = df_blocks["end_time"].max()
    df_log = client.query_log_entries(
        process["start_time"],
        last_end,
        limit=10 * 1024 * 1024,  # request lots of data
        stream_id=stream_id,
    )
    df_log = df_log[df_log["level"] <= int(args.maxlevel)]

    if args.target is not None:
        df_log = df_log[df_log["target"].str.contains(args.target, case=False)]

    if args.last is not None:
        assert args.first is None
        df_log = df_log.tail(int(args.last))
    else:
        limit = (
            args.first
            or 1024 * 1024  # it would be too slow to print it all if it's a large log
        )
        df_log = df_log.head(int(limit))

    print(tabulate(df_log, headers="keys"))


if __name__ == "__main__":
    main()
