#!/usr/bin/env python3
import argparse
import pathlib
import sys
from micromegas.cli import connection
from micromegas.cli.config import ProfileError
from micromegas.cli.version import add_version_argument
import datetime
import micromegas
from tabulate import tabulate


def parse_timestamp(value):
    """Parse a timestamp string into a timezone-aware datetime.

    Accepts RFC 3339 timestamps or relative time deltas like '1h', '30m', '7d'.
    """
    if value is None:
        return None

    # Try parsing as a relative time delta first
    try:
        delta = micromegas.time.parse_time_delta(value)
        return datetime.datetime.now(datetime.timezone.utc) - delta
    except RuntimeError:
        pass

    # Try parsing as an RFC 3339 timestamp
    dt = micromegas.time.parse_datetime(value)
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


def read_sql_source(args):
    """Resolve the SQL text to run from --file (path or '-' for stdin) or args.sql.

    Lets OSError from a bad --file path propagate to the caller uncaught.
    """
    if args.file:
        if args.file == "-":
            sys.stdin.reconfigure(encoding="utf-8")
            sql = sys.stdin.read().strip()
        else:
            sql = pathlib.Path(args.file).read_text(encoding="utf-8").strip()
    else:
        sql = args.sql
    return sql


def main():
    parser = argparse.ArgumentParser(
        prog="micromegas-query",
        description="Run arbitrary SQL queries on the analytics service",
    )
    add_version_argument(parser)
    parser.add_argument(
        "sql", nargs="?", default=None, help="SQL query to execute (or use --file)"
    )
    parser.add_argument(
        "--file",
        help="Read SQL from a file path (use '-' for stdin)",
    )
    parser.add_argument(
        "--begin",
        help="Begin timestamp (RFC 3339 like '2024-01-01T00:00:00Z', or relative like '1h', '30m', '7d')",
    )
    parser.add_argument(
        "--end",
        help="End timestamp (RFC 3339 like '2024-01-01T00:00:00Z', or relative like '1h', '30m', '7d', defaults to now)",
    )
    parser.add_argument(
        "--all",
        action="store_true",
        help="Query the entire time range (no time filtering)",
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
    parser.add_argument(
        "--profile",
        help="Named connection profile from ~/.micromegas/config.json",
    )
    args = parser.parse_args()

    if args.file and args.sql:
        parser.error("cannot use both positional SQL and --file")
    if not args.file and not args.sql:
        parser.error("must provide SQL as a positional argument or via --file")
    try:
        sql = read_sql_source(args)
    except OSError as e:
        parser.error(f"cannot read file '{args.file}': {e}")
    except UnicodeError as e:
        source = "stdin" if args.file == "-" else f"file '{args.file}'"
        parser.error(f"cannot decode {source} as UTF-8: {e}")

    if not args.begin and not args.all:
        parser.error(
            "--begin is required (or use --all to query the entire time range)"
        )
    if args.all and args.begin:
        parser.error("--all and --begin are mutually exclusive")
    if args.all and args.end:
        parser.error("--all and --end are mutually exclusive")

    def parse_timestamp_arg(flag_name, value):
        try:
            return parse_timestamp(value)
        except (ValueError, OverflowError):
            parser.error(
                f"invalid --{flag_name} timestamp '{value}': expected an RFC 3339 "
                f"timestamp (e.g. 2026-07-31T00:00:00Z) or a relative delta like "
                f"'1h', '30m', '7d'"
            )

    begin = parse_timestamp_arg("begin", args.begin)
    end = parse_timestamp_arg("end", args.end)
    if begin is not None and end is None:
        end = datetime.datetime.now(datetime.timezone.utc)

    try:
        client = connection.connect(profile=args.profile)
    except ProfileError as e:
        parser.error(str(e))
    df = client.query(sql, begin, end)

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
