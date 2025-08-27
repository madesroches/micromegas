import argparse
import connection
import datetime
import micromegas


def main():
    parser = argparse.ArgumentParser(
        prog="write_perfetto",
        description="Write span events in perfetto format using server-side generation",
    )
    parser.add_argument("process_id", help="Process UUID to generate trace for")
    parser.add_argument("begin", help="Begin timestamp (ISO format)")
    parser.add_argument("end", help="End timestamp (ISO format)")
    parser.add_argument("filename", help="Output trace file path")
    parser.add_argument(
        "--spans",
        choices=["thread", "async", "both"],
        default="both",
        help="Types of spans to include (default: both)"
    )
    
    args = parser.parse_args()
    begin = datetime.datetime.fromisoformat(args.begin)
    end = datetime.datetime.fromisoformat(args.end)
    client = connection.connect()
    
    # Use the refactored perfetto module instead of duplicating logic
    micromegas.perfetto.write_process_trace_from_chunks(
        client, args.process_id, begin, end, args.spans, args.filename
    )


if __name__ == "__main__":
    main()
