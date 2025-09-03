def write_process_trace_from_chunks(
    client, process_id, begin, end, span_types, trace_filepath
):
    """
    Generate Perfetto trace using server-side perfetto_trace_chunks table function.
    This replaces the old duplicate Python implementation with server-side generation.

    Args:
        client: FlightSQL client
        process_id: Process UUID
        begin: Start time (datetime)
        end: End time (datetime)
        span_types: 'thread', 'async', or 'both'
        trace_filepath: Output file path
    """
    # Convert datetime objects to ISO format strings for SQL
    begin_str = begin.isoformat()
    end_str = end.isoformat()

    # Query chunks using the server-side table function
    # Note: ORDER BY not needed since chunks are naturally produced in order (0, 1, 2, ...)
    sql = f"""
    SELECT chunk_id, chunk_data
    FROM perfetto_trace_chunks(
        '{process_id}',
        '{span_types}',
        TIMESTAMP '{begin_str}',
        TIMESTAMP '{end_str}'
    )
    """

    print(f"Generating {span_types} spans for process {process_id}...")

    # Use streaming interface to process chunks as they arrive
    from tqdm import tqdm

    trace_chunks = []
    expected_chunk_id = 0
    chunk_count = 0

    # We don't know the total number of chunks upfront, so use indeterminate progress
    pbar = tqdm(desc="Processing chunks", unit=" chunks")

    try:
        for record_batch in client.query_stream(sql, begin, end):
            # Convert to pandas for easier access
            df = record_batch.to_pandas()

            # Process each row in the batch
            for _, row in df.iterrows():
                chunk_id = row["chunk_id"]
                chunk_data = row["chunk_data"]

                # Verify chunk ID is the expected sequential value
                if chunk_id != expected_chunk_id:
                    pbar.close()
                    print(
                        f"ERROR: Chunk {chunk_id} received, expected {expected_chunk_id}"
                    )
                    print(f"Chunks may be out of order or missing!")
                    return

                trace_chunks.append(chunk_data)
                expected_chunk_id += 1
                chunk_count += 1
                pbar.update(1)

        pbar.close()

        if chunk_count == 0:
            print(f"No trace data found for process {process_id}")
            return
    except KeyboardInterrupt:
        pbar.close()
        print(f"\nTrace generation interrupted by user after {chunk_count} chunks")
        return
    except Exception as e:
        pbar.close()
        raise

    # Reassemble binary chunks into complete trace
    print(f"Assembling {chunk_count} chunks into trace...")
    trace_bytes = b"".join(trace_chunks)

    print(f"Generated trace with {chunk_count} chunks ({len(trace_bytes)} bytes)")

    # Write to file
    with open(trace_filepath, "wb") as f:
        f.write(trace_bytes)

    print(f"Trace written to {trace_filepath}")


# Main API function with span type selection
def write_process_trace(
    client, process_id, begin, end, trace_filepath, span_types="both"
):
    """
    Generate Perfetto trace with configurable span types.

    Args:
        client: FlightSQL client
        process_id: Process UUID
        begin: Start time (datetime)
        end: End time (datetime)
        trace_filepath: Output file path
        span_types: 'thread', 'async', or 'both' (default: 'both')
    """
    write_process_trace_from_chunks(
        client, process_id, begin, end, span_types, trace_filepath
    )
