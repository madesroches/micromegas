import argparse
import connection
import datetime
import micromegas
import sys
from tabulate import tabulate


def main():
    parser = argparse.ArgumentParser(
        prog="query_process_metrics",
        description="List measures associated with a specific process",
    )
    parser.add_argument("--target")
    parser.add_argument("--name")
    parser.add_argument("--stats", action="store_true")
    parser.add_argument("process_id")
    args = parser.parse_args()
    client = connection.connect()
    df_process = client.find_process(args.process_id)
    if df_process.empty:
        print("process not found")
        sys.exit(1)
    assert df_process.shape[0] == 1
    process = df_process.iloc[0]
    process_id = process["process_id"]
    df_streams = client.query_streams(
        begin=None, end=None, limit=1024, tag_filter="metrics", process_id=process_id
    )
    if df_streams.empty:
        print("metrics stream not found")
        sys.exit(1)
    assert df_streams.shape[0] == 1
    stream = df_streams.iloc[0]
    stream_id = stream["stream_id"]

    df_blocks = client.query_blocks(
        begin=None, end=None, limit=1024, stream_id=stream_id
    )
    if df_blocks.empty:
        print("no metrics entries")
        sys.exit(0)

    last_end = df_blocks["end_time"].max()
    df_metrics = client.query_metrics(
        process["start_time"],
        last_end,
        limit=10 * 1024 * 1024,  # request lots of data
        stream_id=stream_id,
    )

    if args.target is not None:
        df_metrics = df_metrics[
            df_metrics["target"].str.contains(args.target, case=False)
        ]

    if args.name is not None:
        df_metrics = df_metrics[df_metrics["name"].str.contains(args.name, case=False)]

    if args.stats:
        df_metrics = df_metrics.groupby( "name", observed=True )["value"].agg(["count", "min", "mean", "median", "max", "sum"])
    print(df_metrics)


if __name__ == "__main__":
    main()
