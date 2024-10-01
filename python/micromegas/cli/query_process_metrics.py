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
    parser.add_argument("--limit", default=1024)
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
    process_start_time = process["start_time"]
    end = datetime.datetime.now(datetime.timezone.utc)

    conditions = []
    if args.target is not None:
        conditions.append("target ILIKE '%{target}%'".format(target=args.target))

    if args.name is not None:
        conditions.append("name = '{name}'".format(name=args.name))
        
    where = ""
    if len(conditions) > 0:
        where = "WHERE " + "\n AND".join(conditions)

    sql = """
    SELECT *
    FROM view_instance('measures', '{process_id}')
    {where}
    ORDER BY time
    LIMIT {limit}
    ;""".format(
        process_id=process_id,
        limit=int(args.limit),
        where=where
    )

    df_metrics = client.query(
        sql,
        process_start_time,
        end,
    )

    if args.stats:
        df_metrics = df_metrics.groupby("name", observed=True)["value"].agg(
            ["count", "min", "mean", "median", "max", "sum"]
        )
    print(tabulate(df_metrics, headers="keys"))


if __name__ == "__main__":
    main()
