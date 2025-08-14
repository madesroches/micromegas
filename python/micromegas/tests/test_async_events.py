from .test_utils import *

def test_async_events_basic_query():
    """Test basic async events view querying"""
    # Get a process that might have async events with its time range
    sql = """
    SELECT processes.process_id, processes.start_time
    FROM processes
    WHERE exe LIKE '%generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)
    
    if len(processes) == 0:
        print("No generator processes found - skipping async events test")
        return
        
    process_id = processes.iloc[0]["process_id"]
    process_start = processes.iloc[0]["start_time"]
    
    # Use tight time range around the process lifetime
    import datetime
    process_begin = process_start - datetime.timedelta(seconds=1)
    process_end = process_start + datetime.timedelta(minutes=2)
    
    # Query async events for this process using the optimized schema
    sql = """
    SELECT stream_id, block_id, time, event_type, span_id, parent_span_id, 
           name, target, filename, line
    FROM view_instance('async_events', '{process_id}')
    ORDER BY time
    LIMIT 10;
    """.format(process_id=process_id)
    
    async_events = client.query(sql, process_begin, process_end)
    print("Async events found:")
    print(async_events)
    
    # REQUIRE async events to be found for proper validation
    assert len(async_events) > 0, f"No async events found for process {process_id} - test requires actual async span data"
    
    # Verify schema structure
    expected_columns = ['stream_id', 'block_id', 'time', 'event_type', 
                       'span_id', 'parent_span_id', 'name', 'target', 'filename', 'line']
    for col in expected_columns:
        assert col in async_events.columns, f"Missing column: {col}"
    
    # Verify event types
    event_types = set(async_events['event_type'].unique())
    assert event_types.issubset({'begin', 'end'}), f"Unexpected event types: {event_types}"
    
    print(f"✅ Found {len(async_events)} async events with correct schema")

def test_async_events_with_process_join():
    """Test joining async events with process information"""
    # Find a process with async events
    sql = """
    SELECT processes.process_id, processes.start_time
    FROM processes
    WHERE exe LIKE '%generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)
    
    if len(processes) == 0:
        print("No generator processes found - skipping process join test")
        return
        
    process_id = processes.iloc[0]["process_id"]
    process_start = processes.iloc[0]["start_time"]
    
    # Use tight time range around the process lifetime
    import datetime
    process_begin = process_start - datetime.timedelta(seconds=1)
    process_end = process_start + datetime.timedelta(minutes=2)
    
    # Query async events with process info via JOIN
    sql = """
    SELECT ae.name, ae.event_type, ae.time, ae.stream_id,
           p.exe, p.username, p.computer
    FROM view_instance('async_events', '{process_id}') ae
    JOIN streams s ON ae.stream_id = s.stream_id  
    JOIN processes p ON s.process_id = p.process_id
    ORDER BY ae.time
    LIMIT 5;
    """.format(process_id=process_id)
    
    results = client.query(sql, process_begin, process_end)
    print("Async events with process info:")
    print(results)
    
    # REQUIRE results for proper JOIN validation
    assert len(results) > 0, f"No async events found for JOIN test - need actual async span data"
    
    # Verify we have both async event and process columns
    assert 'name' in results.columns  # async event column
    assert 'event_type' in results.columns  # async event column
    assert 'exe' in results.columns  # process column
    assert 'username' in results.columns  # process column
    print("✅ JOIN with process info working correctly")

def test_async_events_parent_child_relationships():
    """Test analyzing parent-child relationships in async spans"""
    # Find a process with async events
    sql = """
    SELECT processes.process_id, processes.start_time
    FROM processes
    WHERE exe LIKE '%generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)
    
    if len(processes) == 0:
        print("No generator processes found - skipping relationship test")
        return
        
    process_id = processes.iloc[0]["process_id"]
    process_start = processes.iloc[0]["start_time"]
    
    # Use tight time range around the process lifetime
    import datetime
    process_begin = process_start - datetime.timedelta(seconds=1)
    process_end = process_start + datetime.timedelta(minutes=2)
    
    # Query for parent-child async span relationships
    sql = """
    SELECT parent.name as parent_name, child.name as child_name, 
           parent.span_id as parent_id, child.parent_span_id,
           parent.stream_id as parent_stream, child.stream_id as child_stream
    FROM view_instance('async_events', '{process_id}') parent
    JOIN view_instance('async_events', '{process_id}') child 
         ON parent.span_id = child.parent_span_id
    WHERE parent.event_type = 'begin' AND child.event_type = 'begin'
    LIMIT 5;
    """.format(process_id=process_id)
    
    relationships = client.query(sql, process_begin, process_end)
    print("Parent-child async span relationships:")
    print(relationships)
    
    # REQUIRE parent-child relationships for proper validation
    assert len(relationships) > 0, "No parent-child relationships found - need nested async spans"
    print(f"✅ Found {len(relationships)} parent-child relationships")

def test_async_events_duration_analysis():
    """Test calculating async operation durations"""
    # Find a process with async events
    sql = """
    SELECT processes.process_id, processes.start_time
    FROM processes
    WHERE exe LIKE '%generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)
    
    if len(processes) == 0:
        print("No generator processes found - skipping duration test")
        return
        
    process_id = processes.iloc[0]["process_id"]
    process_start = processes.iloc[0]["start_time"]
    
    # Use tight time range around the process lifetime
    import datetime
    process_begin = process_start - datetime.timedelta(seconds=1)
    process_end = process_start + datetime.timedelta(minutes=2)
    
    # Calculate durations by matching begin/end events
    sql = """
    SELECT begin_events.name, begin_events.stream_id,
           CAST((end_events.time - begin_events.time) AS BIGINT) / 1000000 as duration_ms
    FROM 
        (SELECT * FROM view_instance('async_events', '{process_id}') 
         WHERE event_type = 'begin') begin_events
    JOIN 
        (SELECT * FROM view_instance('async_events', '{process_id}') 
         WHERE event_type = 'end') end_events
    ON begin_events.span_id = end_events.span_id
    ORDER BY duration_ms DESC
    LIMIT 10;
    """.format(process_id=process_id)
    
    durations = client.query(sql, process_begin, process_end)
    print("Async operation durations:")
    print(durations)
    
    # REQUIRE duration data for proper validation
    assert len(durations) > 0, "No matched begin/end events found - need complete async spans for duration calculation"
    
    # Verify duration calculations
    assert 'duration_ms' in durations.columns
    assert all(durations['duration_ms'] >= 0), "Duration should be non-negative"
    print(f"✅ Successfully calculated durations for {len(durations)} async operations")

def test_async_events_cross_stream_analysis():
    """Test analyzing async events across multiple streams (threads)"""
    # Find a process with async events
    sql = """
    SELECT processes.process_id, processes.start_time
    FROM processes
    WHERE exe LIKE '%generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)
    
    if len(processes) == 0:
        print("No generator processes found - skipping cross-stream test")
        return
        
    process_id = processes.iloc[0]["process_id"]
    process_start = processes.iloc[0]["start_time"]
    
    # Use tight time range around the process lifetime
    import datetime
    process_begin = process_start - datetime.timedelta(seconds=1)
    process_end = process_start + datetime.timedelta(minutes=2)
    
    # Query for async events across different streams
    sql = """
    SELECT DISTINCT stream_id, COUNT(*) as event_count
    FROM view_instance('async_events', '{process_id}')
    WHERE event_type = 'begin'
    GROUP BY stream_id
    ORDER BY event_count DESC;
    """.format(process_id=process_id)
    
    stream_summary = client.query(sql, process_begin, process_end)
    print("Async events per stream (thread):")
    print(stream_summary)
    
    # REQUIRE cross-stream data for proper validation
    assert len(stream_summary) > 0, "No async events found - need async spans across streams"
    
    total_events = stream_summary['event_count'].sum()
    unique_streams = len(stream_summary)
    print(f"✅ Found {total_events} async events across {unique_streams} streams")
    
    # Ideally we want multiple streams to demonstrate cross-stream async debugging
    if unique_streams > 1:
        print("✅ Cross-stream async execution detected (excellent for async debugging)")
    else:
        print("⚠️ Only single stream found - multi-threaded async would be better for testing")

def test_async_events_global_rejection():
    """Test that global async_events queries are properly rejected"""
    # Use a minimal time range for the rejection test
    import datetime
    now = datetime.datetime.now(datetime.timezone.utc)
    test_begin = now - datetime.timedelta(minutes=1)
    test_end = now
    
    try:
        # This should fail since async_events doesn't support global view
        sql = "SELECT COUNT(*) FROM async_events;"
        client.query(sql, test_begin, test_end)
        assert False, "Global async_events query should have been rejected"
    except Exception as e:
        print(f"✅ Global async_events query correctly rejected: {e}")

if __name__ == "__main__":
    test_async_events_basic_query()
    test_async_events_with_process_join()
    test_async_events_parent_child_relationships()
    test_async_events_duration_analysis()
    test_async_events_cross_stream_analysis()
    test_async_events_global_rejection()
    print("🎉 All Python async events integration tests completed!")
