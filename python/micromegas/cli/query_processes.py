import argparse
import connection
import datetime
import micromegas
from tabulate import tabulate


def main():
    parser = argparse.ArgumentParser(
        prog="query_processes",
        description="List processes in the telemetry database",
        epilog="If you are in a corporate environment, you may need to set the MICROMEGAS_PYTHON_MODULE_WRAPPER environment variable to specify the python module responsible to authenticate your requests.",
    )
    parser.add_argument("--since", default="1h", help="[number][m|h|d]")
    parser.add_argument("--limit", default="1000000")
    parser.add_argument("--username")
    parser.add_argument("--exe")
    parser.add_argument("--computer")
    args = parser.parse_args()
    delta = micromegas.time.parse_time_delta(args.since)
    limit = int(args.limit)

    client = connection.connect()
    now = datetime.datetime.now(datetime.timezone.utc)
    begin = now - delta
    end = now
    sql = """
    SELECT process_id, exe, start_time, username, computer, distro, cpu_brand
    FROM processes
    LIMIT {limit}
    ;""".format(limit=limit)
    df_processes = client.query(sql, begin, end)
    if df_processes.empty:
        print("no data")
        return

    if args.username is not None:
        df_processes = df_processes[
            df_processes["username"].str.contains(args.username, case=False)
        ]
    if args.exe is not None:
        df_processes = df_processes[
            df_processes["exe"].str.contains(args.exe, case=False)
        ]
    if args.computer is not None:
        df_processes = df_processes[
            df_processes["computer"].str.contains(args.computer, case=False)
        ]

    df_processes["exe"] = df_processes["exe"].str[-64:]  # keep the 64 rightmost chars
    print(tabulate(df_processes, headers="keys"))


if __name__ == "__main__":
    main()
