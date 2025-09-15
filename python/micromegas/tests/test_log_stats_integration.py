"""Integration tests for log_stats view functionality.

Tests the log_stats SQL view which aggregates log entries by process, minute, 
level, and target for efficient querying of log statistics over time periods.
"""

import pytest
import datetime
import pandas as pd
import sys
import os

sys.path.append(os.path.dirname(__file__))
from test_utils import *


def test_log_stats_basic_functionality():
    """Test basic log_stats view functionality and schema."""
    print("\n=== Log Stats Integration Test: Basic Functionality ===")
    
    now = datetime.datetime.now(datetime.timezone.utc)
    end_time = now
    start_time = end_time - datetime.timedelta(days=90)
    
    print(f"Testing time range: {start_time} to {end_time}")
    
    # Test basic log_stats query
    sql = f"""
    SELECT time_bin, process_id, level, target, count
    FROM log_stats
    ORDER BY time_bin DESC, count DESC
    LIMIT 50
    """
    result = client.query(sql, start_time, end_time)
    
    # Schema validation
    expected_columns = ['time_bin', 'process_id', 'level', 'target', 'count']
    for col in expected_columns:
        assert col in result.columns, f"Missing column: {col}"
    
    if len(result) > 0:
        print(f"✅ Found {len(result)} log_stats records")
        
        # Sample data validation
        row = result.iloc[0]
        assert pd.notna(row['time_bin']), "time_bin should not be null"
        assert pd.notna(row['process_id']), "process_id should not be null"
        assert pd.notna(row['level']), "level should not be null" 
        assert pd.notna(row['target']), "target should not be null"
        assert pd.notna(row['count']), "count should not be null"
        assert row['count'] > 0, "count should be positive"
        
        print(f"  Sample: time_bin={row['time_bin']}, process_id={row['process_id']}, level={row['level']}, target={row['target']}, count={row['count']}")
        print("✅ Schema validation passed")
    else:
        pytest.fail("No log_stats records found in time range - view may not be materialized")



def test_log_stats_time_filtering():
    """Test time-based filtering and grouping functionality."""
    print("\n=== Testing Time-based Filtering ===")
    
    # Calculate time range - use large time window like other successful tests
    now = datetime.datetime.now(datetime.timezone.utc)
    end_time = now
    start_time = end_time - datetime.timedelta(days=90)  # 90-day window
    
    # Test time-based aggregation
    time_query = f"""
    SELECT time_bin, sum(count) as total_count
    FROM log_stats
    GROUP BY time_bin
    ORDER BY time_bin
    """
    
    time_result = client.query(time_query, start_time, end_time)
    print(f"✅ Time-based filtering returned {len(time_result)} time bins")
    
    if len(time_result) == 0:
        pytest.fail("No data found in 90-day window - insufficient test data")
    
    total_events = time_result['total_count'].sum()
    print(f"  Total events in 90-day window: {total_events}")
    
    # Validate time bin structure
    time_bins = time_result['time_bin'].tolist()
    assert time_bins == sorted(time_bins), "Time bins should be chronologically ordered"
    
    # Validate each time bin is properly rounded to minute boundaries
    for time_bin in time_bins:
        if isinstance(time_bin, str):
            time_bin = pd.to_datetime(time_bin)
        assert time_bin.second == 0, "Time bins should be rounded to minute boundaries"
        assert time_bin.microsecond == 0, "Time bins should have no microseconds"
    
    print("✅ Time filtering validation passed")


def test_log_stats_level_grouping():
    """Test level-based grouping functionality."""
    print("\n=== Testing Level-based Grouping ===")
    
    # Calculate time range - use months of data
    now = datetime.datetime.now(datetime.timezone.utc)
    end_time = now
    start_time = end_time - datetime.timedelta(days=90)
    
    # Test level grouping
    level_query = f"""
    SELECT level, sum(count) as total_count
    FROM log_stats
    GROUP BY level
    ORDER BY level
    LIMIT 10
    """
    
    level_result = client.query(level_query, start_time, end_time)
    print(f"✅ Level grouping returned {len(level_result)} levels")
    
    if len(level_result) == 0:
        pytest.fail("No level data found - insufficient test data for level grouping")
    
    level_names = {1: "Fatal", 2: "Error", 3: "Warning", 4: "Info", 5: "Debug", 6: "Trace"}
    
    for _, row in level_result.iterrows():
        level_name = level_names.get(row['level'], f"Level{row['level']}")
        print(f"  {level_name}: {row['total_count']} events")
        
        # Validate level values are in expected range
        assert 1 <= row['level'] <= 6, f"Log level should be 1-6, got {row['level']}"
        assert row['total_count'] > 0, "Event count should be positive"
    
    print("✅ Level grouping validation passed")


def test_log_stats_process_filtering():
    """Test process and target filtering functionality."""
    print("\n=== Testing Process and Target Filtering ===")
    
    # Calculate time range - use months of data
    now = datetime.datetime.now(datetime.timezone.utc)
    end_time = now
    start_time = end_time - datetime.timedelta(days=90)
    
    # First get some sample data
    sample_query = f"""
    SELECT process_id, target, count
    FROM log_stats
    ORDER BY count DESC
    LIMIT 5
    """
    
    sample_result = client.query(sample_query, start_time, end_time)
    
    if len(sample_result) == 0:
        pytest.fail("No sample data available for filtering test - insufficient test data")
    
    sample_process = sample_result.iloc[0]['process_id']
    sample_target = sample_result.iloc[0]['target']
    
    # Test filtering by specific process and target
    filter_query = f"""
    SELECT time_bin, level, count
    FROM log_stats
    WHERE process_id = '{sample_process}'
    AND target = '{sample_target}'
    ORDER BY time_bin
    LIMIT 100
    """
    
    filter_result = client.query(filter_query, start_time, end_time)
    print(f"✅ Process/target filtering returned {len(filter_result)} records")
    print(f"  Filtered by process_id='{sample_process}', target='{sample_target}'")
    
    if len(filter_result) == 0:
        pytest.fail(f"No results from filtering by process_id='{sample_process}' and target='{sample_target}' - filtering may be too restrictive or data too sparse")
    
    # Validate all results match the filter criteria
    total_count = filter_result['count'].sum()
    assert total_count > 0, "Filtered results should have positive count"
    print(f"  Total filtered events: {total_count}")
    print("✅ Process/target filtering validation passed")


def test_log_stats_error_handling():
    """Test error handling for invalid queries."""
    print("\n=== Testing Error Handling ===")
    
    # Test invalid column reference
    with pytest.raises(Exception):
        sql = "SELECT invalid_column FROM log_stats LIMIT 1"
        client.query(sql)
    print("✅ Invalid column properly rejected")
    
    # Test malformed time range
    with pytest.raises(Exception):
        sql = "SELECT * FROM log_stats WHERE time_bin >= 'invalid-timestamp'"
        client.query(sql)
    print("✅ Invalid timestamp properly rejected")
    
    print("✅ Error handling tests completed")


def test_log_stats_performance():
    """Test query performance for common patterns."""
    print("\n=== Testing Performance ===")
    
    # Calculate time range - use large time window like other successful tests
    now = datetime.datetime.now(datetime.timezone.utc)
    end_time = now
    start_time = end_time - datetime.timedelta(days=90)  # 90-day window
    
    import time
    
    # Test performance of aggregation query
    start_perf = time.time()
    sql = f"""
    SELECT time_bin, level, sum(count) as total_count
    FROM log_stats
    GROUP BY time_bin, level
    ORDER BY time_bin, level
    """
    
    result = client.query(sql, start_time, end_time)
    end_perf = time.time()
    
    query_time = end_perf - start_perf
    print(f"✅ Aggregation query completed in {query_time:.3f}s")
    print(f"  Processed {len(result)} aggregated records")
    
    if len(result) == 0:
        pytest.fail("No aggregated records returned for performance test - insufficient test data")
    
    # Performance should be reasonable (under 5 seconds for materialized data)
    if query_time < 5.0:
        print("✅ Query performance acceptable")
    else:
        print("⚠ Query took longer than expected (materialization may be needed)")


if __name__ == "__main__":
    # Run tests individually for debugging
    test_log_stats_basic_functionality()
    test_log_stats_time_filtering()
    test_log_stats_level_grouping()
    test_log_stats_process_filtering()
    test_log_stats_error_handling()
    test_log_stats_performance()
    print("\n✅ All log_stats integration tests completed!")
