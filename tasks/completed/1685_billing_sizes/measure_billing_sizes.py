"""Measure per-event billing sizes for the SaaS cost comparison (#1685).

Per-GB vendors bill on the enriched, uncompressed document they receive, not on
Micromegas's compact storage. This samples log entries, metric points and
thread spans, serializes each as a compact JSON document carrying its resource
context (exe, username, computer, process properties), and reports aggregate
byte sizes only. Rows are never printed or written anywhere.

Usage (from python/micromegas):
    poetry run python ../../tasks/completed/1685_billing_sizes/measure_billing_sizes.py --profile <name>
"""

import argparse
import datetime
import json
import statistics

from micromegas.connection import connect_with_profile

WINDOW_OFFSETS_HOURS = [2, 27, 55, 81, 109, 139]
ROWS_PER_WINDOW = 5000
SPAN_WINDOW = datetime.timedelta(minutes=2)
PROCESSES_PER_WINDOW = 2

RESOURCE_COLUMNS = (
    "process_id, exe, username, computer, "
    "jsonb_format_json(process_properties) AS process_properties"
)
LOG_SQL = f"""
SELECT time, target, level, msg, jsonb_format_json(properties) AS properties, {RESOURCE_COLUMNS}
FROM log_entries LIMIT {ROWS_PER_WINDOW}
"""
METRIC_SQL = f"""
SELECT time, target, name, unit, value, jsonb_format_json(properties) AS properties, {RESOURCE_COLUMNS}
FROM measures LIMIT {ROWS_PER_WINDOW}
"""
BUSY_CPU_PROCESSES_SQL = f"""
SELECT process_id, SUM(nb_objects) AS events
FROM blocks
WHERE array_has("streams.tags", 'cpu')
GROUP BY process_id ORDER BY events DESC LIMIT {PROCESSES_PER_WINDOW}
"""


def scalar(df):
    value = df.iloc[0, 0]
    return 0 if value is None or value != value else int(value)


def json_field(value):
    if value is None:
        return None
    try:
        return json.loads(value)
    except (TypeError, ValueError):
        return value


def doc_size(row):
    doc = {}
    for key, value in row.items():
        if key in ("properties", "process_properties"):
            value = json_field(value)
        elif hasattr(value, "isoformat"):
            value = value.isoformat()
        doc[key] = value
    return len(json.dumps(doc, separators=(",", ":"), default=str).encode("utf-8"))


def sizes_of(df):
    return [doc_size(row) for row in df.to_dict(orient="records")]


def summarize(label, sizes):
    if not sizes:
        print(f"{label}: no rows sampled")
        return
    qs = statistics.quantiles(sizes, n=20)
    print(
        f"{label}: n={len(sizes)} mean={statistics.fmean(sizes):.0f} B "
        f"p50={statistics.median(sizes):.0f} B p5={qs[0]:.0f} B p95={qs[-1]:.0f} B"
    )


def measure_spans(client, begin, end):
    sizes, span_count, event_count = [], 0, 0
    busy = client.query(BUSY_CPU_PROCESSES_SQL, begin, end)
    for process_id in busy["process_id"]:
        resource = client.query(
            'SELECT process_id, "processes.exe" AS exe, "processes.username" AS username, '
            '"processes.computer" AS computer, '
            'jsonb_format_json("processes.properties") AS process_properties '
            f"FROM blocks WHERE process_id = '{process_id}' LIMIT 1",
            begin,
            end,
        ).to_dict(orient="records")[0]
        spans = client.query(
            'SELECT thread_name, id, parent, depth, begin, "end", duration, name, target, filename, line '
            f"FROM process_spans('{process_id}', 'thread') LIMIT {ROWS_PER_WINDOW}",
            begin,
            end,
        )
        sizes += [
            doc_size({**row, **resource}) for row in spans.to_dict(orient="records")
        ]
        # Count spans and stream events over the same window; blocks straddling its
        # edges make the ratio approximate.
        span_count += scalar(
            client.query(
                f"SELECT COUNT(*) AS n FROM process_spans('{process_id}', 'thread') "
                f"WHERE begin >= '{begin.isoformat()}' AND \"end\" <= '{end.isoformat()}'",
                begin,
                end,
            )
        )
        event_count += scalar(
            client.query(
                "SELECT SUM(nb_objects) AS n FROM blocks "
                f"WHERE process_id = '{process_id}' AND array_has(\"streams.tags\", 'cpu') "
                f"AND begin_time >= '{begin.isoformat()}' AND end_time <= '{end.isoformat()}'",
                begin,
                end,
            )
        )
    return sizes, span_count, event_count


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--profile", required=True)
    args = parser.parse_args()
    client = connect_with_profile(args.profile)

    now = datetime.datetime.now(datetime.timezone.utc).replace(
        minute=0, second=0, microsecond=0
    )
    log_sizes, metric_sizes, span_sizes = [], [], []
    spans_total, events_total = 0, 0
    for offset in WINDOW_OFFSETS_HOURS:
        begin = now - datetime.timedelta(hours=offset)
        end = begin + datetime.timedelta(hours=1)
        log_sizes += sizes_of(client.query(LOG_SQL, begin, end))
        metric_sizes += sizes_of(client.query(METRIC_SQL, begin, end))
        span_end = begin + SPAN_WINDOW
        sizes, spans, events = measure_spans(client, begin, span_end)
        span_sizes += sizes
        spans_total += spans
        events_total += events

    summarize("log entry", log_sizes)
    summarize("metric point", metric_sizes)
    summarize("span", span_sizes)
    if spans_total:
        print(
            f"cpu stream events per thread span: {events_total / spans_total:.2f} "
            f"(spans={spans_total}, events={events_total})"
        )


if __name__ == "__main__":
    main()
