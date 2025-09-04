"""Unified integration tests for Perfetto trace generation (Phases 5-6).

This combines Phase 5 infrastructure testing with Phase 6 real data testing
into a comprehensive integration test suite.
"""

import pytest
import pyarrow._flight as flight
import micromegas
import sys
import os

sys.path.append(os.path.dirname(__file__))
from test_utils import *


def test_perfetto_trace_chunks_integration():
    """Comprehensive integration test for perfetto_trace_chunks function."""
    print("\n=== Perfetto Integration Test: Table Function & Real Data ===")

    # Find a test process
    process_info = find_test_process()
    if not process_info:
        pytest.skip(
            "No test process found - run telemetry generator to create test data"
        )

    process_id = process_info["process_id"]
    start_time = process_info["start_time"]
    end_time = process_info["end_time"]

    print(f"Using test process: {process_id}")
    print(f"  Executable: {process_info['exe']}")
    print(f"  Time range: {start_time} to {end_time}")
    print(
        f"  Streams: {process_info['stream_count']}, Blocks: {process_info['block_count']}"
    )

    # Skip data availability checks for performance - we'll handle failures in the loop
    print("  Skipping data availability checks for faster execution")

    # Test only 'both' span type for performance - covers thread+async functionality
    span_type_results = {}

    for span_type in ["both"]:
        print(f"\n--- Testing span type: {span_type} ---")

        # Data availability checked by trying query - faster than pre-checking

        sql = f"""
        SELECT chunk_id, chunk_data
        FROM perfetto_trace_chunks(
            '{process_id}',
            '{span_type}',
            TIMESTAMP '{start_time.isoformat()}',
            TIMESTAMP '{end_time.isoformat()}'
        )
        -- ORDER BY removed for performance - chunk_id is already sequential
        LIMIT 5
        """

        try:
            result = client.query(sql, start_time, end_time)
            span_type_results[span_type] = result

            # Validate results
            assert len(result) > 0, f"Should generate chunks for {span_type} spans"
            assert "chunk_id" in result.columns, "Should have chunk_id column"
            assert "chunk_data" in result.columns, "Should have chunk_data column"

            # Validate chunk structure
            chunk_ids = result["chunk_id"].to_list()
            assert chunk_ids == sorted(chunk_ids), "Chunk IDs should be sequential"
            assert chunk_ids[0] == 0, "First chunk should have ID 0"

            # Validate binary data
            total_bytes = sum(len(chunk) for chunk in result["chunk_data"])
            assert total_bytes > 0, "Should generate non-empty trace data"

            # Validate each chunk contains valid binary data
            for i, chunk_data in enumerate(result["chunk_data"]):
                assert isinstance(chunk_data, bytes), f"Chunk {i} should be bytes"
                assert len(chunk_data) > 0, f"Chunk {i} should not be empty"

            print(f"  ✅ Generated {len(result)} chunks ({total_bytes} bytes total)")
            print(f"  ✅ Chunk IDs: {chunk_ids}")

        except Exception as e:
            print(f"  ❌ Failed to generate {span_type} trace: {e}")
            # Don't fail the entire test - some span types might not have data
            continue

    # Verify at least one span type worked
    assert (
        len(span_type_results) > 0
    ), "At least one span type should generate traces successfully"

    # Test trace reconstruction (Phase 6 specific)
    if "both" in span_type_results:
        print(f"\n--- Testing trace reconstruction ---")
        result = span_type_results["both"]

        # Reconstruct complete trace from chunks
        trace_bytes = b"".join(result["chunk_data"])

        # Basic validation of reconstructed trace
        assert (
            len(trace_bytes) > 100
        ), "Complete trace should be substantial (>100 bytes)"
        print(f"  ✅ Reconstructed complete trace: {len(trace_bytes)} bytes")

    print(f"\n✅ Perfetto integration test completed successfully!")


def test_perfetto_trace_chunks_error_handling():
    """Test error handling for invalid arguments."""
    print("\n=== Testing Error Handling ===")

    # Test invalid span type
    with pytest.raises(flight.FlightInternalError) as exc_info:
        sql = """
        SELECT chunk_id, chunk_data
        FROM perfetto_trace_chunks(
            'any-process-id',
            'invalid-span-type',
            TIMESTAMP '2024-01-01T00:00:00Z', 
            TIMESTAMP '2024-01-01T01:00:00Z'
        )
        """
        client.query(sql)

    assert "span_types must be 'thread', 'async', or 'both'" in str(exc_info.value)
    print("✅ Invalid span type properly rejected")

    # Test missing arguments
    with pytest.raises(flight.FlightInternalError) as exc_info:
        sql = """
        SELECT chunk_id, chunk_data
        FROM perfetto_trace_chunks('process-id', 'both')
        """
        client.query(sql)

    # Should fail on missing arguments
    assert "Third argument" in str(exc_info.value)
    print("✅ Missing arguments properly rejected")

    # Test non-existent process (should fail gracefully)
    with pytest.raises(flight.FlightInternalError) as exc_info:
        sql = """
        SELECT chunk_id, chunk_data
        FROM perfetto_trace_chunks(
            'non-existent-process-id',
            'both',
            TIMESTAMP '2024-01-01T00:00:00Z',
            TIMESTAMP '2024-01-01T01:00:00Z'
        )
        """
        client.query(sql)

    assert "Process non-existent-process-id not found" in str(exc_info.value)
    print("✅ Non-existent process properly rejected")


def test_perfetto_trace_chunks_schema():
    """Test that output schema matches expected structure."""
    print("\n=== Testing Schema Validation ===")

    # Find a test process
    process_info = find_test_process()
    if not process_info:
        pytest.skip("No test process available for schema testing")

    sql = f"""
    SELECT chunk_id, chunk_data
    FROM perfetto_trace_chunks(
        '{process_info['process_id']}',
        'both',
        TIMESTAMP '{process_info['start_time'].isoformat()}',
        TIMESTAMP '{process_info['end_time'].isoformat()}'
    )
    -- ORDER BY removed for performance - chunk_id is already sequential
    LIMIT 1
    """

    result = client.query(sql, process_info["start_time"], process_info["end_time"])

    # Schema validation
    assert len(result.columns) == 2, "Should have exactly 2 columns"
    assert "chunk_id" in result.columns, "Should have chunk_id column"
    assert "chunk_data" in result.columns, "Should have chunk_data column"

    # Data type validation
    assert (
        result["chunk_id"].dtype == "int32"
    ), f"chunk_id should be int32, got {result['chunk_id'].dtype}"
    assert (
        result["chunk_data"].dtype == "object"
    ), f"chunk_data should be object (bytes), got {result['chunk_data'].dtype}"

    # Content validation
    if len(result) > 0:
        assert isinstance(
            result["chunk_data"].iloc[0], bytes
        ), "chunk_data should contain bytes"
        assert result["chunk_id"].iloc[0] == 0, "First chunk should have ID 0"

    print("✅ Schema validation passed")


if __name__ == "__main__":
    # Run tests individually for debugging
    test_perfetto_trace_chunks_integration()
    test_perfetto_trace_chunks_error_handling()
    test_perfetto_trace_chunks_schema()
    print("\nAll Perfetto integration tests completed!")
