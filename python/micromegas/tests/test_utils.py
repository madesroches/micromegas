import datetime
import pandas as pd
import micromegas

client = micromegas.connect()

now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(days=10000)
end = now + datetime.timedelta(hours=1)
limit = 1024


def find_test_process():
    """Find a recent process with telemetry data for testing."""
    sql = """
    SELECT 
        b.process_id,
        MIN(b."processes.exe") as exe,
        MIN(b.begin_time) as start_time,
        MAX(b.end_time) as end_time,
        COUNT(DISTINCT b.stream_id) as stream_count,
        COUNT(DISTINCT b.block_id) as block_count,
        SUM(b.nb_objects) as total_objects
    FROM blocks b
    WHERE b."processes.exe" LIKE '%telemetry-generator%' 
    GROUP BY b.process_id
    HAVING COUNT(DISTINCT b.block_id) > 0
    ORDER BY MAX(b.end_time) DESC
    LIMIT 1
    """

    result = client.query(sql)
    if result.empty:
        return None

    row = result.iloc[0]
    return {
        "process_id": row["process_id"],
        "exe": row["exe"],
        "start_time": row["start_time"],
        "end_time": row["end_time"],
        "stream_count": row["stream_count"],
        "block_count": row["block_count"],
        "total_objects": row["total_objects"],
    }


def check_process_data_availability(process_id, start_time, end_time):
    """Check what types of telemetry data are available for a process."""
    checks = {}

    # Check for thread streams - LIMIT 1 for faster existence check
    sql_thread_streams = f"""
    SELECT COUNT(*) as count
    FROM streams
    WHERE process_id = '{process_id}'
      AND array_has(tags, 'cpu')
    LIMIT 1
    """
    thread_streams = client.query(sql_thread_streams)
    checks["thread_streams"] = (
        thread_streams["count"].iloc[0] if not thread_streams.empty else 0
    )

    # Check for async events - use LIMIT for faster check
    try:
        sql_async_events = f"""
        SELECT COUNT(*) as count
        FROM view_instance('async_events', '{process_id}')
        LIMIT 1
        """
        async_events = client.query(sql_async_events, start_time, end_time)
        checks["async_events"] = (
            async_events["count"].iloc[0] if not async_events.empty else 0
        )
    except Exception:
        checks["async_events"] = 0

    return checks
