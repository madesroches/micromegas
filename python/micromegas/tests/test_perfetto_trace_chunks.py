"""Integration tests for perfetto_trace_chunks table function."""

from .test_utils import *
import micromegas


def test_perfetto_trace_chunks_basic():
    """Test basic perfetto trace chunk generation."""
    # Note: This is a Phase 5 infrastructure test with dummy data
    # Phase 6 will implement the actual trace generation logic
    
    sql = """
    SELECT chunk_id, length(chunk_data) as chunk_size
    FROM perfetto_trace_chunks(
        'dummy-process-id',
        'both', 
        TIMESTAMP '2024-01-01T00:00:00Z',
        TIMESTAMP '2024-01-01T01:00:00Z'
    )
    ORDER BY chunk_id
    """
    
    result = client.query(sql)
    
    # Phase 5: Should return dummy chunks from the implementation
    assert len(result) > 0, "Should return at least one chunk"
    
    # Verify chunk structure
    assert 'chunk_id' in result.columns
    assert 'chunk_size' in result.columns
    
    # Verify chunks are sequential
    chunk_ids = result['chunk_id'].to_list()
    assert chunk_ids == sorted(chunk_ids), "Chunk IDs should be sequential"
    assert chunk_ids[0] == 0, "First chunk should have ID 0"
    
    print(f"✓ Generated {len(result)} chunks with sizes: {result['chunk_size'].to_list()}")


def test_perfetto_trace_chunks_span_types():
    """Test different span type parameters."""
    for span_type in ['thread', 'async', 'both']:
        sql = f"""
        SELECT chunk_id
        FROM perfetto_trace_chunks(
            'dummy-process-id',
            '{span_type}', 
            TIMESTAMP '2024-01-01T00:00:00Z',
            TIMESTAMP '2024-01-01T01:00:00Z'
        )
        """
        
        result = client.query(sql)
        chunk_count = len(result)
        
        assert chunk_count > 0, f"Should return chunks for span_type '{span_type}'"
        print(f"✓ Span type '{span_type}' returned {chunk_count} chunks")


def test_perfetto_trace_chunks_invalid_args():
    """Test error handling for invalid arguments."""
    import pyarrow._flight as flight
    
    # Test invalid span type
    try:
        sql = """
        SELECT chunk_id, chunk_data
        FROM perfetto_trace_chunks(
            'process-id',
            'invalid-span-type',
            TIMESTAMP '2024-01-01T00:00:00Z', 
            TIMESTAMP '2024-01-01T01:00:00Z'
        )
        """
        client.query(sql)
        assert False, "Should have raised an error for invalid span type"
    except flight.FlightInternalError as e:
        assert "span_types must be 'thread', 'async', or 'both'" in str(e)
        print("✓ Invalid span type properly rejected")
    
    # Test missing arguments
    try:
        sql = """
        SELECT chunk_id, chunk_data
        FROM perfetto_trace_chunks('process-id', 'both')
        """
        client.query(sql)
        assert False, "Should have raised an error for missing arguments"
    except flight.FlightInternalError as e:
        assert "Third argument" in str(e)  # It fails on the missing 3rd arg before checking 4th
        print("✓ Missing arguments properly rejected")


def test_perfetto_trace_chunks_schema():
    """Test that the output schema is correct."""
    sql = """
    SELECT chunk_id, chunk_data
    FROM perfetto_trace_chunks(
        'test-process',
        'both',
        TIMESTAMP '2024-01-01T00:00:00Z',
        TIMESTAMP '2024-01-01T01:00:00Z'
    )
    LIMIT 1
    """
    
    result = client.query(sql)
    
    # Verify schema structure
    assert len(result.columns) == 2, "Should have exactly 2 columns"
    assert 'chunk_id' in result.columns
    assert 'chunk_data' in result.columns
    
    # Verify data types
    assert result['chunk_id'].dtype == 'int32', f"chunk_id should be int32, got {result['chunk_id'].dtype}"
    assert result['chunk_data'].dtype == 'object', f"chunk_data should be object (bytes), got {result['chunk_data'].dtype}"
    
    # Verify data content
    assert len(result) == 1, "LIMIT 1 should return exactly 1 row"
    assert isinstance(result['chunk_data'].iloc[0], bytes), "chunk_data should contain bytes"
    
    print("✓ Schema validation passed")


if __name__ == "__main__":
    test_perfetto_trace_chunks_basic()
    test_perfetto_trace_chunks_span_types()
    test_perfetto_trace_chunks_invalid_args()
    test_perfetto_trace_chunks_schema()
    print("All perfetto_trace_chunks tests passed!")