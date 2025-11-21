import argparse
import connection
import datetime
import micromegas
from tabulate import tabulate


def parse_timestamp(value):
    """Parse a timestamp string into a timezone-aware datetime.

    Accepts ISO format strings or relative time deltas like '1h', '30m', '7d'.
    """
    if value is None:
        return None

    # Try parsing as a relative time delta first
    try:
        delta = micromegas.time.parse_time_delta(value)
        return datetime.datetime.now(datetime.timezone.utc) - delta
    except RuntimeError:
        pass

    # Try parsing as ISO format
    dt = datetime.datetime.fromisoformat(value)
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=datetime.timezone.utc)
    return dt


def truncate_value(value, max_width):
    """Truncate a value to max_width characters."""
    if value is None:
        return value
    s = str(value)
    if len(s) > max_width:
        return s[: max_width - 3] + "..."
    return s


def main():
    parser = argparse.ArgumentParser(
        prog="query",
        description="Run arbitrary SQL queries on the analytics service",
        epilog="If you are in a corporate environment, you may need to set the MICROMEGAS_PYTHON_MODULE_WRAPPER environment variable to specify the python module responsible to authenticate your requests.",
    )
    parser.add_argument("sql", help="SQL query to execute")
    parser.add_argument(
        "--begin",
        help="Begin timestamp (ISO format or relative like '1h', '30m', '7d')",
    )
    parser.add_argument(
        "--end",
        help="End timestamp (ISO format or relative like '1h', '30m', '7d')",
    )
    parser.add_argument(
        "--format",
        choices=["table", "csv", "json"],
        default="table",
        help="Output format (default: table)",
    )
    parser.add_argument(
        "--max-colwidth",
        type=int,
        default=50,
        help="Maximum column width for table format (default: 50, 0 for unlimited)",
    )
    args = parser.parse_args()

    begin = parse_timestamp(args.begin)
    end = parse_timestamp(args.end)

    client = connection.connect()
    df = client.query(args.sql, begin, end)

    if df.empty:
        print("no data")
        return

    if args.format == "table":
        # Truncate column values if max_colwidth is set
        if args.max_colwidth > 0:
            for col in df.columns:
                df[col] = df[col].apply(lambda x: truncate_value(x, args.max_colwidth))
        print(tabulate(df, headers="keys", showindex=False, tablefmt="simple"))
    elif args.format == "csv":
        print(df.to_csv(index=False))
    elif args.format == "json":
        print(df.to_json(orient="records", indent=2))


if __name__ == "__main__":
    main()
