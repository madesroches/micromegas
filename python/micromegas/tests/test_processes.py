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
    sql = "select properties, property_get(properties, 'build-version') from processes WHERE array_length(properties) > 0 LIMIT 10;"
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
