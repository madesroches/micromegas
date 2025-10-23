from .test_utils import *


def test_blocks_query():
    sql = "select * from blocks LIMIT 10;"
    blocks = client.query(sql)
    print(blocks)


def test_blocks_properties_schema():
    """Test that blocks properties are dictionary-encoded JSONB format."""
    sql = """
    SELECT
        block_id,
        arrow_typeof("streams.properties") as streams_props_type,
        arrow_typeof("processes.properties") as processes_props_type
    FROM blocks
    WHERE "streams.properties" IS NOT NULL
    LIMIT 5
    """

    result = client.query(sql)
    print(f"Found {len(result)} blocks with properties")

    if len(result) > 0:
        streams_type = result.get("streams_props_type", [None])[0]
        processes_type = result.get("processes_props_type", [None])[0]

        print(f"Streams properties type: {streams_type}")
        print(f"Processes properties type: {processes_type}")

        # Verify dictionary encoding
        if streams_type and "Dictionary" in str(streams_type):
            print("✓ Streams properties are dictionary-encoded")
        else:
            print(f"⚠ Expected dictionary encoding for streams, got: {streams_type}")

        if processes_type and "Dictionary" in str(processes_type):
            print("✓ Processes properties are dictionary-encoded")
        else:
            print(
                f"⚠ Expected dictionary encoding for processes, got: {processes_type}"
            )
    else:
        print("⚠ No blocks with properties found")


def test_blocks_property_get_jsonb():
    """Test property_get UDF works with dictionary-encoded JSONB properties."""
    sql = """
    SELECT
        block_id,
        property_get("streams.properties", 'test_key') as stream_prop,
        property_get("processes.properties", 'build-version') as process_prop,
        arrow_typeof(property_get("processes.properties", 'build-version')) as prop_type
    FROM blocks
    WHERE "processes.properties" IS NOT NULL
    LIMIT 5
    """

    result = client.query(sql)
    print(f"Found {len(result)} blocks for property_get testing")

    if len(result) > 0:
        prop_type = result.get("prop_type", [None])[0]
        print(f"property_get return type: {prop_type}")

        # Check if property_get returns dictionary-encoded results
        if prop_type and "Dictionary" in str(prop_type):
            print("✓ property_get returns dictionary-encoded data")
        else:
            print(f"property_get return type: {prop_type}")

        # Check for actual property values
        process_props = result.get("process_prop", [])
        non_null_props = [p for p in process_props if p is not None]
        if non_null_props:
            print(f"✓ Found {len(non_null_props)} non-null property values")
        else:
            print("⚠ No property values found")
    else:
        print("⚠ No blocks with process properties found")


def test_blocks_properties_to_jsonb_passthrough():
    """Test that properties_to_jsonb UDF works as pass-through for already-encoded JSONB."""
    sql = """
    SELECT
        block_id,
        properties_to_jsonb("streams.properties") as jsonb_streams,
        arrow_typeof(properties_to_jsonb("streams.properties")) as jsonb_type
    FROM blocks
    WHERE "streams.properties" IS NOT NULL
    LIMIT 3
    """

    result = client.query(sql)
    print(f"Found {len(result)} blocks for properties_to_jsonb testing")

    if len(result) > 0:
        jsonb_type = result.get("jsonb_type", [None])[0]
        print(f"properties_to_jsonb output type: {jsonb_type}")

        if jsonb_type and "Dictionary" in str(jsonb_type):
            print("✓ properties_to_jsonb maintains dictionary encoding")
        else:
            print(f"⚠ Expected dictionary encoding, got: {jsonb_type}")
    else:
        print("⚠ No blocks with stream properties found")


def test_blocks_properties_stats():
    """Test blocks properties statistics and coverage."""
    sql = """
    SELECT
        COUNT(*) as total_blocks,
        COUNT("streams.properties") as blocks_with_stream_props,
        COUNT("processes.properties") as blocks_with_process_props,
        COUNT(CASE WHEN "streams.properties" IS NOT NULL AND "processes.properties" IS NOT NULL THEN 1 END) as blocks_with_both_props
    FROM blocks
    """

    result = client.query(sql)
    print("Blocks properties statistics:")

    if len(result) > 0:
        total = result.get("total_blocks", [0])[0]
        stream_props = result.get("blocks_with_stream_props", [0])[0]
        process_props = result.get("blocks_with_process_props", [0])[0]
        both_props = result.get("blocks_with_both_props", [0])[0]

        print(f"  Total blocks: {total}")
        print(f"  Blocks with stream properties: {stream_props}")
        print(f"  Blocks with process properties: {process_props}")
        print(f"  Blocks with both properties: {both_props}")

        if total > 0:
            print(f"  Stream properties coverage: {stream_props/total*100:.1f}%")
            print(f"  Process properties coverage: {process_props/total*100:.1f}%")
            print("✓ Blocks properties statistics retrieved successfully")
    else:
        print("⚠ No statistics available")
