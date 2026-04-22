from .test_utils import *


def _find_process_with_net_stream():
    """Find the most recent process that has at least one 'net'-tagged stream."""
    sql = """
    SELECT DISTINCT p.process_id, p.start_time
    FROM processes p
    JOIN streams s ON s.process_id = p.process_id
    WHERE array_has(s.tags, 'net')
    ORDER BY p.start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)
    if len(processes) == 0:
        return None, None
    return processes.iloc[0]["process_id"], processes.iloc[0]["start_time"]


def test_net_spans_basic_query():
    """Basic net_spans view query: rows returned and schema is sane."""
    process_id, process_start = _find_process_with_net_stream()
    if process_id is None:
        print("No process with a 'net'-tagged stream found - skipping test")
        return

    import datetime

    process_begin = process_start - datetime.timedelta(seconds=1)
    process_end = process_start + datetime.timedelta(minutes=10)

    sql = """
    SELECT process_id, stream_id, span_id, parent_span_id, depth, kind, name,
           connection_name, is_outgoing, begin_bits, end_bits, bit_size, begin_time, end_time
    FROM view_instance('net_spans', '{process_id}')
    ORDER BY begin_time
    LIMIT 50;
    """.format(
        process_id=process_id
    )

    rows = client.query(sql, process_begin, process_end)
    print("Net spans found:")
    print(rows.head())

    assert (
        len(rows) > 0
    ), f"No net spans found for process {process_id} - test requires captured net traffic"

    expected_columns = [
        "process_id",
        "stream_id",
        "span_id",
        "parent_span_id",
        "depth",
        "kind",
        "name",
        "connection_name",
        "is_outgoing",
        "begin_bits",
        "end_bits",
        "bit_size",
        "begin_time",
        "end_time",
    ]
    for col in expected_columns:
        assert col in rows.columns, f"Missing column: {col}"

    kinds = set(rows["kind"].unique())
    assert kinds.issubset(
        {"connection", "object", "property", "rpc"}
    ), f"Unexpected kinds: {kinds}"

    # end_bits - begin_bits must equal bit_size for every row.
    delta = rows["end_bits"] - rows["begin_bits"]
    assert (
        delta == rows["bit_size"]
    ).all(), "end_bits - begin_bits must equal bit_size for every net span row"
    print(f"✅ {len(rows)} net spans returned with correct schema and bit invariants")


def test_net_spans_inclusive_size_invariant():
    """Each Connection's bit_size must be >= sum of direct-child bit sizes."""
    process_id, process_start = _find_process_with_net_stream()
    if process_id is None:
        print("No process with a 'net'-tagged stream found - skipping invariant test")
        return

    import datetime

    process_begin = process_start - datetime.timedelta(seconds=1)
    process_end = process_start + datetime.timedelta(minutes=10)

    # For every Connection row, compare its bit_size to the sum of the bit_size
    # of its direct children. Since bit_size is inclusive and children stack
    # beneath it, parent_bits must be >= sum(child_bits).
    sql = """
    WITH spans AS (
        SELECT span_id, parent_span_id, kind, bit_size
        FROM view_instance('net_spans', '{process_id}')
    )
    SELECT p.span_id AS conn_span_id,
           p.bit_size AS parent_bits,
           COALESCE(SUM(c.bit_size), 0) AS child_bits
    FROM spans p
    LEFT JOIN spans c ON c.parent_span_id = p.span_id
    WHERE p.kind = 'connection'
    GROUP BY p.span_id, p.bit_size
    LIMIT 100;
    """.format(
        process_id=process_id
    )

    summary = client.query(sql, process_begin, process_end)
    print("Connection bit rollups:")
    print(summary.head())

    if len(summary) == 0:
        print("No connection spans — skipping invariant check")
        return

    bad = summary[summary["parent_bits"] < summary["child_bits"]]
    assert (
        len(bad) == 0
    ), f"inclusive-size invariant violated for {len(bad)} connection span(s):\n{bad}"
    print(f"✅ inclusive-size invariant holds for {len(summary)} connection span(s)")


def test_net_spans_global_rejection():
    """`view_instance('net_spans', 'global')` must be rejected."""
    import datetime

    now = datetime.datetime.now(datetime.timezone.utc)
    test_begin = now - datetime.timedelta(minutes=1)
    test_end = now

    sql = "SELECT COUNT(*) FROM view_instance('net_spans', 'global');"
    rejected = False
    try:
        client.query(sql, test_begin, test_end)
    except Exception as e:
        rejected = True
        print(f"✅ Global net_spans query correctly rejected: {e}")
    assert rejected, "Global net_spans query should have been rejected"


if __name__ == "__main__":
    test_net_spans_basic_query()
    test_net_spans_inclusive_size_invariant()
    test_net_spans_global_rejection()
    print("🎉 All Python net_spans integration tests completed!")
