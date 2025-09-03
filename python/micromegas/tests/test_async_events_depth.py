"""
Python Integration Tests for Async Span Depth Tracking

These tests validate the end-to-end functionality of depth tracking in async span events,
covering the complete flow from event generation to storage and querying via FlightSQL.

Based on the async_span_depth_tracking_plan.md implementation.
"""

import datetime
import pandas as pd
import sys
import os

# Add the parent directory to the path to import micromegas
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.dirname(__file__))))
import micromegas

client = micromegas.connect()


def test_async_events_depth_field_present():
    """Test that depth field is present in async events schema"""
    # Get a process that might have async events
    sql = """
    SELECT processes.process_id, processes.start_time
    FROM processes
    WHERE exe LIKE '%generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)

    assert (
        len(processes) > 0
    ), "No generator processes found - test requires generator process with async events"

    process_id = processes.iloc[0]["process_id"]
    process_start = processes.iloc[0]["start_time"]

    # Use tight time range around the process lifetime
    process_begin = process_start - datetime.timedelta(seconds=1)
    process_end = process_start + datetime.timedelta(minutes=2)

    # Query async events specifically checking for depth field
    sql = """
    SELECT stream_id, event_type, span_id, parent_span_id, depth, name
    FROM view_instance('async_events', '{process_id}')
    ORDER BY time
    LIMIT 5;
    """.format(
        process_id=process_id
    )

    async_events = client.query(sql, process_begin, process_end)
    print("Async events with depth field:")
    print(async_events)

    # REQUIRE async events to validate depth field
    assert (
        len(async_events) > 0
    ), f"No async events found for process {process_id} - test requires actual async span data"

    # Verify depth field is present and has expected characteristics
    assert "depth" in async_events.columns, "Missing depth field in async events schema"

    # Verify depth values are non-negative integers
    depths = async_events["depth"]
    assert all(
        depths >= 0
    ), f"Depth values should be non-negative, found: {depths.tolist()}"
    assert depths.dtype.name in [
        "uint32",
        "int64",
    ], f"Depth should be integer type, found: {depths.dtype}"

    print(f"✅ Found depth field with values: {sorted(depths.unique())}")


def test_async_events_depth_hierarchy_validation():
    """Test that depth values correctly represent call hierarchy nesting"""
    # Get a process with async events
    sql = """
    SELECT processes.process_id, processes.start_time
    FROM processes
    WHERE exe LIKE '%generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)

    assert (
        len(processes) > 0
    ), "No generator processes found - test requires generator process with async events"

    process_id = processes.iloc[0]["process_id"]
    process_start = processes.iloc[0]["start_time"]

    process_begin = process_start - datetime.timedelta(seconds=1)
    process_end = process_start + datetime.timedelta(minutes=2)

    # Query parent-child relationships with depth validation
    sql = """
    SELECT parent.span_id as parent_id, parent.depth as parent_depth,
           child.span_id as child_id, child.depth as child_depth,
           parent.name as parent_name, child.name as child_name
    FROM view_instance('async_events', '{process_id}') parent
    JOIN view_instance('async_events', '{process_id}') child
         ON parent.span_id = child.parent_span_id
    WHERE parent.event_type = 'begin' AND child.event_type = 'begin'
    ORDER BY parent.depth, child.depth
    LIMIT 20;
    """.format(
        process_id=process_id
    )

    relationships = client.query(sql, process_begin, process_end)
    print("Parent-child depth relationships:")
    print(relationships)

    assert (
        len(relationships) > 0
    ), "No parent-child relationships found - test requires nested async operations"

    # Verify hierarchy constraint: child depth should be parent depth + 1
    for _, row in relationships.iterrows():
        parent_depth = row["parent_depth"]
        child_depth = row["child_depth"]
        expected_child_depth = parent_depth + 1

        assert child_depth == expected_child_depth, (
            f"Invalid depth hierarchy: parent {row['parent_name']} (depth {parent_depth}) "
            f"should have child {row['child_name']} at depth {expected_child_depth}, "
            f"but found depth {child_depth}"
        )

    print(f"✅ Validated {len(relationships)} parent-child depth relationships")


def test_async_events_depth_based_filtering():
    """Test filtering async events by depth level"""
    # Get a process with async events
    sql = """
    SELECT processes.process_id, processes.start_time
    FROM processes
    WHERE exe LIKE '%generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)

    assert (
        len(processes) > 0
    ), "No generator processes found - test requires generator process with async events"

    process_id = processes.iloc[0]["process_id"]
    process_start = processes.iloc[0]["start_time"]

    process_begin = process_start - datetime.timedelta(seconds=1)
    process_end = process_start + datetime.timedelta(minutes=2)

    # Test different depth-based filters

    # 1. Top-level operations (depth 0)
    sql_top_level = """
    SELECT name, depth, COUNT(*) as count
    FROM view_instance('async_events', '{process_id}')
    WHERE event_type = 'begin' AND depth = 0
    GROUP BY name, depth
    ORDER BY count DESC;
    """.format(
        process_id=process_id
    )

    top_level = client.query(sql_top_level, process_begin, process_end)
    print("Top-level async operations (depth = 0):")
    print(top_level)

    # 2. Shallow operations (depth <= 2)
    sql_shallow = """
    SELECT depth, COUNT(*) as event_count
    FROM view_instance('async_events', '{process_id}')
    WHERE event_type = 'begin' AND depth <= 2
    GROUP BY depth
    ORDER BY depth;
    """.format(
        process_id=process_id
    )

    shallow = client.query(sql_shallow, process_begin, process_end)
    print("Shallow async operations (depth <= 2):")
    print(shallow)

    # 3. Deep operations (depth >= 3)
    sql_deep = """
    SELECT name, depth, COUNT(*) as count
    FROM view_instance('async_events', '{process_id}')
    WHERE event_type = 'begin' AND depth >= 3
    GROUP BY name, depth
    ORDER BY depth DESC, count DESC
    LIMIT 10;
    """.format(
        process_id=process_id
    )

    deep = client.query(sql_deep, process_begin, process_end)
    print("Deep async operations (depth >= 3):")
    print(deep)

    # Verify filtering works correctly
    if len(shallow) > 0:
        max_shallow_depth = shallow["depth"].max()
        assert (
            max_shallow_depth <= 2
        ), f"Shallow filter failed: found depth {max_shallow_depth}"
        print(f"✅ Shallow depth filtering working (max depth: {max_shallow_depth})")

    if len(deep) > 0:
        min_deep_depth = deep["depth"].min()
        assert min_deep_depth >= 3, f"Deep filter failed: found depth {min_deep_depth}"
        print(f"✅ Deep depth filtering working (min depth: {min_deep_depth})")

    print("✅ Depth-based filtering validation completed")


def test_async_events_depth_performance_analysis():
    """Test using depth for performance analysis as outlined in the plan"""
    # Get a process with async events
    sql = """
    SELECT processes.process_id, processes.start_time
    FROM processes
    WHERE exe LIKE '%generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)

    assert (
        len(processes) > 0
    ), "No generator processes found - test requires generator process with async events"

    process_id = processes.iloc[0]["process_id"]
    process_start = processes.iloc[0]["start_time"]

    process_begin = process_start - datetime.timedelta(seconds=1)
    process_end = process_start + datetime.timedelta(minutes=2)

    # Performance analysis query from the plan
    sql = """
    SELECT name, AVG(duration_ms) as avg_duration, COUNT(*) as count, depth
    FROM (
      SELECT
        begin_events.name,
        begin_events.depth,
        CAST((end_events.time - begin_events.time) AS BIGINT) / 1000000 as duration_ms
      FROM
        (SELECT * FROM view_instance('async_events', '{process_id}') WHERE event_type = 'begin') begin_events
      LEFT JOIN
        (SELECT * FROM view_instance('async_events', '{process_id}') WHERE event_type = 'end') end_events
        ON begin_events.span_id = end_events.span_id
      WHERE end_events.span_id IS NOT NULL
    )
    WHERE depth < 3  -- Only shallow operations (top-level and immediate children)
    GROUP BY name, depth
    ORDER BY avg_duration DESC;
    """.format(
        process_id=process_id
    )

    performance_results = client.query(sql, process_begin, process_end)
    print("Performance analysis by depth (shallow operations):")
    print(performance_results)

    # Compare performance by call depth query from the plan
    sql_depth_comparison = """
    SELECT depth, COUNT(*) as span_count, AVG(duration_ms) as avg_duration
    FROM (
      SELECT
        begin_events.depth,
        CAST((end_events.time - begin_events.time) AS BIGINT) / 1000000 as duration_ms
      FROM
        (SELECT * FROM view_instance('async_events', '{process_id}') WHERE event_type = 'begin') begin_events
      LEFT JOIN
        (SELECT * FROM view_instance('async_events', '{process_id}') WHERE event_type = 'end') end_events
        ON begin_events.span_id = end_events.span_id
      WHERE end_events.span_id IS NOT NULL
    )
    GROUP BY depth
    ORDER BY depth;
    """.format(
        process_id=process_id
    )

    depth_comparison = client.query(sql_depth_comparison, process_begin, process_end)
    print("Performance comparison by call depth:")
    print(depth_comparison)

    # REQUIRE some performance data for validation
    assert (
        len(performance_results) > 0 or len(depth_comparison) > 0
    ), "No matched begin/end events found - performance analysis requires complete async spans"

    if len(performance_results) > 0:
        assert "avg_duration" in performance_results.columns
        assert "depth" in performance_results.columns
        assert all(
            performance_results["avg_duration"] >= 0
        ), "Duration should be non-negative"
        print(
            f"✅ Performance analysis working for {len(performance_results)} operations"
        )

    if len(depth_comparison) > 0:
        assert "depth" in depth_comparison.columns
        assert "span_count" in depth_comparison.columns
        assert "avg_duration" in depth_comparison.columns
        print(
            f"✅ Depth performance comparison working across {len(depth_comparison)} depth levels"
        )


def test_async_events_depth_nested_operations():
    """Test identifying operations that spawn many nested async calls"""
    # Get a process with async events
    sql = """
    SELECT processes.process_id, processes.start_time
    FROM processes
    WHERE exe LIKE '%generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)

    assert (
        len(processes) > 0
    ), "No generator processes found - test requires generator process with async events"

    process_id = processes.iloc[0]["process_id"]
    process_start = processes.iloc[0]["start_time"]

    process_begin = process_start - datetime.timedelta(seconds=1)
    process_end = process_start + datetime.timedelta(minutes=2)

    # Query from the plan: Find operations that spawn many nested async calls
    sql = """
    SELECT name, depth, COUNT(*) as nested_count
    FROM view_instance('async_events', '{process_id}')
    WHERE depth > 0 AND event_type = 'begin'
    GROUP BY name, depth
    HAVING COUNT(*) > 1  -- Functions that create multiple nested async operations
    ORDER BY nested_count DESC, depth DESC
    LIMIT 10;
    """.format(
        process_id=process_id
    )

    nested_operations = client.query(sql, process_begin, process_end)
    print("Operations with many nested async calls:")
    print(nested_operations)

    # Additional analysis: Depth distribution
    sql_distribution = """
    SELECT depth, COUNT(*) as operation_count
    FROM view_instance('async_events', '{process_id}')
    WHERE event_type = 'begin'
    GROUP BY depth
    ORDER BY depth;
    """.format(
        process_id=process_id
    )

    depth_distribution = client.query(sql_distribution, process_begin, process_end)
    print("Async operation depth distribution:")
    print(depth_distribution)

    # Validation
    assert (
        len(nested_operations) > 0 or len(depth_distribution) > 0
    ), "No async operations found - test requires async span data"

    if len(nested_operations) > 0:
        assert "depth" in nested_operations.columns
        assert "nested_count" in nested_operations.columns
        assert all(
            nested_operations["depth"] > 0
        ), "Should only include nested operations (depth > 0)"
        print(f"✅ Found {len(nested_operations)} types of nested async operations")

    if len(depth_distribution) > 0:
        assert "depth" in depth_distribution.columns
        assert "operation_count" in depth_distribution.columns
        max_depth = depth_distribution["depth"].max()
        total_operations = depth_distribution["operation_count"].sum()
        print(
            f"✅ Depth distribution: {total_operations} operations with max depth {max_depth}"
        )


def test_async_events_depth_range_validation():
    """Test that depth values are within expected ranges"""
    # Get a process with async events
    sql = """
    SELECT processes.process_id, processes.start_time
    FROM processes
    WHERE exe LIKE '%generator%'
    ORDER BY start_time DESC
    LIMIT 1;
    """
    processes = client.query(sql)

    assert (
        len(processes) > 0
    ), "No generator processes found - test requires generator process with async events"

    process_id = processes.iloc[0]["process_id"]
    process_start = processes.iloc[0]["start_time"]

    process_begin = process_start - datetime.timedelta(seconds=1)
    process_end = process_start + datetime.timedelta(minutes=2)

    # Check depth value distribution and ensure they are reasonable
    sql = """
    SELECT MIN(depth) as min_depth, MAX(depth) as max_depth, 
           AVG(depth) as avg_depth, COUNT(DISTINCT depth) as unique_depths
    FROM view_instance('async_events', '{process_id}')
    WHERE event_type = 'begin';
    """.format(
        process_id=process_id
    )

    depth_stats = client.query(sql, process_begin, process_end)
    print("Depth value statistics:")
    print(depth_stats)

    if len(depth_stats) > 0 and depth_stats.iloc[0]["min_depth"] is not None:
        min_depth = depth_stats.iloc[0]["min_depth"]
        max_depth = depth_stats.iloc[0]["max_depth"]

        # Validate depth ranges
        assert (
            min_depth >= 0
        ), f"Minimum depth should be non-negative, found: {min_depth}"
        assert max_depth < 1000, f"Maximum depth seems unreasonably high: {max_depth}"

        print(f"✅ Depth values in valid range: {min_depth} to {max_depth}")
    else:
        print("⚠️ No depth statistics available")


if __name__ == "__main__":
    print("🧪 Running Python Integration Tests for Async Span Depth Tracking")
    print("=" * 70)

    test_async_events_depth_field_present()
    print()

    test_async_events_depth_hierarchy_validation()
    print()

    test_async_events_depth_based_filtering()
    print()

    test_async_events_depth_performance_analysis()
    print()

    test_async_events_depth_nested_operations()
    print()

    test_async_events_depth_range_validation()
    print()

    print("🎉 All Python async events depth integration tests completed!")
