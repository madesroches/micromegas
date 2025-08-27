import argparse
import datetime
import micromegas
import os
try:
    import connection
except ImportError:
    from . import connection


def get_process_time_range(client, process_id):
    """Get the time range for a process from the database."""
    sql = f"SELECT start_time, last_block_end_time FROM processes WHERE process_id = '{process_id}'"
    result = client.query(sql)
    
    if result.empty:
        raise ValueError(f"Process {process_id} not found")
    
    row = result.iloc[0]
    # Convert pandas Timestamps to Python datetime objects
    start_time = row['start_time'].to_pydatetime()
    
    # Handle case where last_block_end_time might be None
    if row['last_block_end_time'] is not None:
        end_time = row['last_block_end_time'].to_pydatetime()
    else:
        # Use current time if no end time recorded
        end_time = datetime.datetime.now(datetime.timezone.utc)
    
    return start_time, end_time


def main():
    parser = argparse.ArgumentParser(
        prog="write_perfetto",
        description="Write span events in perfetto format using server-side generation",
    )
    parser.add_argument("process_id", help="Process UUID to generate trace for")
    parser.add_argument("--begin", help="Begin timestamp (ISO format, optional - uses process start time if not specified)")
    parser.add_argument("--end", help="End timestamp (ISO format, optional - uses process end time if not specified)")
    parser.add_argument("--filename", help="Output trace file path (optional - defaults to {process_id}.perfetto)")
    parser.add_argument(
        "--spans",
        choices=["thread", "async", "both"],
        default="both",
        help="Types of spans to include (default: both)"
    )
    
    args = parser.parse_args()
    client = connection.connect()
    
    # Get process time range if begin/end not specified
    if not args.begin or not args.end:
        process_start, process_end = get_process_time_range(client, args.process_id)
        begin = datetime.datetime.fromisoformat(args.begin) if args.begin else process_start
        end = datetime.datetime.fromisoformat(args.end) if args.end else process_end
    else:
        begin = datetime.datetime.fromisoformat(args.begin)
        end = datetime.datetime.fromisoformat(args.end)
    
    # Default filename if not specified
    filename = args.filename if args.filename else f"{args.process_id}.perfetto"
    
    print(f"Generating trace for process {args.process_id}")
    print(f"Time range: {begin} to {end}")
    print(f"Output file: {filename}")
    
    # Use the refactored perfetto module instead of duplicating logic
    micromegas.perfetto.write_process_trace_from_chunks(
        client, args.process_id, begin, end, args.spans, filename
    )


if __name__ == "__main__":
    main()
