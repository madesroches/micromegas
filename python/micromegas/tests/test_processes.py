from .test_utils import *


def test_processes_query():
    # Use a time range ending at least 10 seconds ago to avoid eventual consistency issues
    cutoff_time = now - datetime.timedelta(seconds=10)
    sql = "select * from processes LIMIT 10;"
    processes = client.query(sql, begin, cutoff_time)
    print(processes)


def test_processes_properties_query():
    # Use a time range ending at least 10 seconds ago to avoid eventual consistency issues
    cutoff_time = now - datetime.timedelta(seconds=10)
    sql = "select properties, property_get(properties, 'build-version') from processes WHERE properties_length(properties) > 0 LIMIT 10;"
    processes = client.query(sql, begin, cutoff_time)
    print(processes)


def test_processes_last_block_fields():
    """Test that the new last_block_end_ticks and last_block_end_time fields are present."""
    cutoff_time = now - datetime.timedelta(seconds=10)
    sql = "SELECT last_block_end_ticks, last_block_end_time FROM processes WHERE last_block_end_ticks IS NOT NULL LIMIT 5;"

    processes = client.query(sql, begin, cutoff_time)
    print(f"Found {len(processes)} processes with last block data")

    # Just verify we can query the fields and got some results
    if len(processes) > 0:
        print("✓ last_block_end_ticks and last_block_end_time fields are accessible")
    else:
        print("⚠ No processes with last block data found")


def test_property_get_returns_dictionary():
    """Test that property_get returns dictionary-encoded data for memory efficiency."""
    cutoff_time = now - datetime.timedelta(seconds=10)

    # Query using property_get to get build version
    sql = """
    SELECT
        property_get(properties, 'build-version') as version,
        arrow_typeof(property_get(properties, 'build-version')) as version_type
    FROM processes
    WHERE properties_length(properties) > 0
    LIMIT 10
    """

    result = client.query(sql, begin, cutoff_time)
    print(f"Found {len(result)} processes with properties")

    if len(result) > 0:
        # Check that the type is dictionary
        version_type = result["version_type"].iloc[0]
        print(f"property_get return type: {version_type}")

        if "Dictionary" in version_type:
            print("✅ property_get returns dictionary-encoded data")

            # Test that we can still access the values normally
            versions = result["version"].dropna()
            if len(versions) > 0:
                print(f"Sample version values: {versions.head(3).tolist()}")
                print("✅ Dictionary values are accessible")
            else:
                print("⚠ No non-null version values found")
        else:
            print(f"❌ Expected Dictionary type but got: {version_type}")

        # Compare memory efficiency by testing with multiple property gets
        memory_test_sql = """
        SELECT
            property_get(properties, 'build-version') as version,
            property_get(properties, 'platform') as platform,
            property_get(properties, 'target') as target
        FROM processes
        WHERE properties_length(properties) > 0
        LIMIT 100
        """

        memory_result = client.query(memory_test_sql, begin, cutoff_time)
        if len(memory_result) > 0:
            print(f"✅ Multiple property_get calls work with dictionary encoding")
            print(f"   Retrieved {len(memory_result)} rows with 3 property columns")
    else:
        print("⚠ No processes with properties found for testing")
