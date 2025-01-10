import argparse
import connection
import datetime
import micromegas
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
    parser.add_argument("--msg")
    parser.add_argument("--maxlevel", default=6)
    parser.add_argument("process_id")
    args = parser.parse_args()

    client = connection.connect()
    sort_order = ""
    limit = 1024
    conditions = []

    if args.target is not None:
        conditions.append("AND target ILIKE '%{target}%'".format(target=args.target))

    if args.msg is not None:
        conditions.append("AND msg ILIKE '%{msg}%'".format(msg=args.msg))

    if args.first is not None:
        sort_order = "ASC"
        limit = int(args.first)

    if args.last is not None:
        sort_order = "DESC"
        limit = int(args.last)
        
    sql = """
    SELECT *
    FROM view_instance('log_entries', '{process_id}')
    WHERE level <= {max_level}
    {conditions}
    ORDER BY time {sort_order}
    LIMIT {limit}
    ;""".format(
        process_id=args.process_id,
        sort_order=sort_order,
        max_level=int(args.maxlevel),
        limit=limit,
        conditions="\n".join(conditions),
    )
    df_log = client.query(sql).sort_values('time')
    print(tabulate(df_log, headers="keys"))


if __name__ == "__main__":
    main()
