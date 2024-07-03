import argparse
import datetime
import importlib
import micromegas
import os
from tabulate import tabulate


def main():
    parser = argparse.ArgumentParser(
        prog="query_processes",
        description="List processes in the telemetry database",
        epilog="If you are in a corporate environment, you may need to set the MICROMEGAS_PYTHON_MODULE_WRAPPER environment variable to specify the python module responsible to authenticate your requests.",
    )
    parser.add_argument("--since", default="1h", help="[number][m|h|d]")
    parser.add_argument("--limit", default="1024")
    args = parser.parse_args()
    delta = micromegas.time.parse_time_delta(args.since)
    limit = int(args.limit)

    micromegas_module_name = os.environ.get(
        "MICROMEGAS_PYTHON_MODULE_WRAPPER", "micromegas"
    )
    micromegas_module = importlib.import_module(micromegas_module_name)
    client = micromegas_module.connect()
    now = datetime.datetime.now(datetime.timezone.utc)
    begin = now - delta
    end = now
    df_processes = client.query_processes(begin, end, limit)
    if df_processes.empty:
        print("no data")
        return
    df_processes = df_processes[
        [
            "process_id",
            "exe",
            "start_time",
            "username",
            "computer",
            "distro",
            "cpu_brand",
        ]
    ]
    df_processes["exe"] = df_processes["exe"].str[-64:] #keep the 64 rightmost chars
    print(tabulate(df_processes, headers="keys"))


if __name__ == "__main__":
    main()
