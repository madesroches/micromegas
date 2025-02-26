import argparse
import connection
import datetime
import micromegas
import sys
from tabulate import tabulate


def main():
    parser = argparse.ArgumentParser(
        prog="write_perfetto",
        description="Write span events in perfetto format",
    )
    parser.add_argument("process_id")
    parser.add_argument("begin")
    parser.add_argument("end")
    parser.add_argument("filename")
    args = parser.parse_args()
    begin = datetime.datetime.fromisoformat(args.begin)
    end = datetime.datetime.fromisoformat(args.end)
    client = connection.connect()
    micromegas.perfetto.write_process_trace(client, args.process_id, begin, end, args.filename)


if __name__ == "__main__":
    main()
