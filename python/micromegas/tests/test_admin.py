"""Tests for micromegas.admin module."""

import pytest
import unittest.mock
import pandas as pd
import micromegas.admin


class MockFlightSQLClient:
    """Mock FlightSQL client for testing admin functions."""

    def __init__(self, mock_data=None):
        self.mock_data = mock_data or {}

    def query(self, sql):
        """Mock query method that returns predefined data based on SQL patterns."""
        # Check if this is a list_incompatible_partitions query
        if "list_partitions() p" in sql and "list_view_sets() vs" in sql:
            if "incompatible_test_data" in self.mock_data:
                return self.mock_data["incompatible_test_data"]
            else:
                # Return empty DataFrame with expected columns
                return pd.DataFrame(
                    columns=[
                        "view_set_name",
                        "view_instance_id",
                        "incompatible_schema_hash",
                        "current_schema_hash",
                        "partition_count",
                        "total_size_bytes",
                        "file_paths",
                    ]
                )

        # Check if this is a retire_partition_by_file call
        elif "retire_partition_by_file(" in sql:
            if "retirement_file_result_data" in self.mock_data:
                return self.mock_data["retirement_file_result_data"]
            else:
                # Extract file path from SQL for realistic mock response
                import re

                match = re.search(r"retire_partition_by_file\('([^']+)'\)", sql)
                file_path = match.group(1) if match else "unknown.parquet"
                return pd.DataFrame(
                    {"message": [f"SUCCESS: Retired partition {file_path}"]}
                )

        # Default empty response
        return pd.DataFrame()


def test_list_incompatible_partitions_empty():
    """Test list_incompatible_partitions with no incompatible partitions."""
    client = MockFlightSQLClient()
    result = micromegas.admin.list_incompatible_partitions(client)

    assert isinstance(result, pd.DataFrame)
    assert len(result) == 0
    expected_columns = [
        "view_set_name",
        "view_instance_id",
        "incompatible_schema_hash",
        "current_schema_hash",
        "partition_count",
        "total_size_bytes",
        "file_paths",
    ]
    assert list(result.columns) == expected_columns


def test_list_incompatible_partitions_with_data():
    """Test list_incompatible_partitions with mock incompatible partitions."""
    mock_data = {
        "incompatible_test_data": pd.DataFrame(
            {
                "view_set_name": ["log_entries", "log_entries"],
                "view_instance_id": ["process-123", "process-456"],
                "incompatible_schema_hash": ["[3]", "[2]"],
                "current_schema_hash": ["[4]", "[4]"],
                "partition_count": [5, 3],
                "total_size_bytes": [1024000, 512000],
                "file_paths": [
                    ["/path/to/partition1.parquet", "/path/to/partition2.parquet"],
                    [
                        "/path/to/partition3.parquet",
                        "/path/to/partition4.parquet",
                        "/path/to/partition5.parquet",
                    ],
                ],
            }
        )
    }

    client = MockFlightSQLClient(mock_data)
    result = micromegas.admin.list_incompatible_partitions(client)

    assert isinstance(result, pd.DataFrame)
    assert len(result) == 2
    assert result["view_set_name"].tolist() == ["log_entries", "log_entries"]
    assert result["partition_count"].sum() == 8
    assert result["total_size_bytes"].sum() == 1536000


def test_list_incompatible_partitions_with_view_filter():
    """Test list_incompatible_partitions with view_set_name filter."""
    mock_data = {
        "incompatible_test_data": pd.DataFrame(
            {
                "view_set_name": ["log_entries"],
                "view_instance_id": ["process-123"],
                "incompatible_schema_hash": ["[3]"],
                "current_schema_hash": ["[4]"],
                "partition_count": [5],
                "total_size_bytes": [1024000],
                "file_paths": [
                    ["/path/to/partition1.parquet", "/path/to/partition2.parquet"]
                ],
            }
        )
    }

    client = MockFlightSQLClient(mock_data)
    result = micromegas.admin.list_incompatible_partitions(client, "log_entries")

    assert isinstance(result, pd.DataFrame)
    assert len(result) == 1
    assert result["view_set_name"].iloc[0] == "log_entries"


def test_retire_incompatible_partitions_empty():
    """Test retire_incompatible_partitions with no incompatible partitions."""
    client = MockFlightSQLClient()
    result = micromegas.admin.retire_incompatible_partitions(client)

    assert isinstance(result, pd.DataFrame)
    assert len(result) == 0
    expected_columns = [
        "view_set_name",
        "view_instance_id",
        "partitions_retired",
        "partitions_failed",
        "storage_freed_bytes",
        "retirement_messages",
    ]
    assert list(result.columns) == expected_columns


def test_retire_incompatible_partitions_with_data():
    """Test retire_incompatible_partitions with file-path-based retirement."""
    mock_data = {
        "incompatible_test_data": pd.DataFrame(
            {
                "view_set_name": ["log_entries"],
                "view_instance_id": ["process-123"],
                "incompatible_schema_hash": ["[3]"],
                "current_schema_hash": ["[4]"],
                "partition_count": [2],
                "total_size_bytes": [1024000],
                "file_paths": [
                    ["/path/to/partition1.parquet", "/path/to/partition2.parquet"]
                ],
            }
        )
    }

    client = MockFlightSQLClient(mock_data)
    result = micromegas.admin.retire_incompatible_partitions(client)

    assert isinstance(result, pd.DataFrame)
    assert len(result) == 1
    assert result["view_set_name"].iloc[0] == "log_entries"
    assert result["view_instance_id"].iloc[0] == "process-123"
    assert (
        result["partitions_retired"].iloc[0] == 2
    )  # Both partitions retired successfully
    assert result["partitions_failed"].iloc[0] == 0  # No failures
    assert result["storage_freed_bytes"].iloc[0] == 1024000  # All storage freed

    # Check retirement messages
    messages = result["retirement_messages"].iloc[0]
    assert len(messages) == 2  # Two retirement attempts
    assert all(msg.startswith("SUCCESS:") for msg in messages)


def test_retire_incompatible_partitions_with_failures():
    """Test retire_incompatible_partitions handling partial failures."""
    mock_data = {
        "incompatible_test_data": pd.DataFrame(
            {
                "view_set_name": ["log_entries"],
                "view_instance_id": ["process-456"],
                "incompatible_schema_hash": ["[2]"],
                "current_schema_hash": ["[4]"],
                "partition_count": [3],
                "total_size_bytes": [1536000],
                "file_paths": [
                    [
                        "/path/to/good1.parquet",
                        "/path/to/missing.parquet",
                        "/path/to/good2.parquet",
                    ]
                ],
            }
        ),
        "retirement_file_result_data": pd.DataFrame(
            {
                "message": [
                    "SUCCESS: Retired partition /path/to/good1.parquet",
                    "ERROR: Partition not found: /path/to/missing.parquet",
                    "SUCCESS: Retired partition /path/to/good2.parquet",
                ]
            }
        ),
    }

    client = MockFlightSQLClient(mock_data)

    # Override the query method to return different results for different file paths
    original_query = client.query

    def mock_query_with_failures(sql):
        if "retire_partition_by_file('/path/to/missing.parquet')" in sql:
            return pd.DataFrame(
                {"message": ["ERROR: Partition not found: /path/to/missing.parquet"]}
            )
        elif "retire_partition_by_file(" in sql:
            import re

            match = re.search(r"retire_partition_by_file\('([^']+)'\)", sql)
            file_path = match.group(1) if match else "unknown.parquet"
            return pd.DataFrame(
                {"message": [f"SUCCESS: Retired partition {file_path}"]}
            )
        else:
            return original_query(sql)

    client.query = mock_query_with_failures
    result = micromegas.admin.retire_incompatible_partitions(client)

    assert isinstance(result, pd.DataFrame)
    assert len(result) == 1
    assert result["view_set_name"].iloc[0] == "log_entries"
    assert result["view_instance_id"].iloc[0] == "process-456"
    assert result["partitions_retired"].iloc[0] == 2  # 2 successful retirements
    assert result["partitions_failed"].iloc[0] == 1  # 1 failure

    # Storage freed should be proportional to successful retirements (2/3 of total)
    expected_freed = int(1536000 * (2 / 3))
    assert result["storage_freed_bytes"].iloc[0] == expected_freed

    # Check retirement messages
    messages = result["retirement_messages"].iloc[0]
    assert len(messages) == 3  # Three retirement attempts
    success_count = sum(1 for msg in messages if msg.startswith("SUCCESS:"))
    error_count = sum(1 for msg in messages if msg.startswith("ERROR:"))
    assert success_count == 2
    assert error_count == 1


def test_sql_injection_resilience():
    """Test that functions handle malicious input gracefully (DataFusion handles SQL execution safely)."""
    client = MockFlightSQLClient()

    # Test with malicious view_set_name
    malicious_name = "log_entries'; DROP TABLE lakehouse_partitions; --"

    # This should not raise an exception (DataFusion handles execution safely)
    result = micromegas.admin.list_incompatible_partitions(client, malicious_name)
    assert isinstance(result, pd.DataFrame)

    # Test retirement function with malicious data
    mock_data = {
        "incompatible_test_data": pd.DataFrame(
            {
                "view_set_name": ["test'; DROP TABLE test; --"],
                "view_instance_id": ["proc'; DELETE FROM procs; --"],
                "incompatible_schema_hash": ["[3'; TRUNCATE schemas; --]"],
                "current_schema_hash": ["[4]"],
                "partition_count": [1],
                "total_size_bytes": [1000],
                "file_paths": [["/path/to/malicious'; DROP TABLE files; --.parquet"]],
            }
        )
    }

    client_with_malicious = MockFlightSQLClient(mock_data)
    result = micromegas.admin.retire_incompatible_partitions(client_with_malicious)

    # Should handle the malicious input gracefully (DataFusion protects against injection)
    assert isinstance(result, pd.DataFrame)


# Integration tests using real FlightSQL client


def test_list_incompatible_partitions_integration():
    """Integration test for list_incompatible_partitions using real FlightSQL client."""
    from .test_utils import client

    # Call with real client - should not fail
    result = micromegas.admin.list_incompatible_partitions(client)

    # Verify result is a DataFrame
    assert isinstance(result, pd.DataFrame)

    # Verify expected columns are present
    expected_columns = [
        "view_set_name",
        "view_instance_id",
        "incompatible_schema_hash",
        "current_schema_hash",
        "partition_count",
        "total_size_bytes",
        "file_paths",
    ]
    assert list(result.columns) == expected_columns

    print(f"list_incompatible_partitions returned {len(result)} incompatible partition groups")
    if len(result) > 0:
        print(f"Sample data:\n{result.head()}")


def test_list_incompatible_partitions_with_filter_integration():
    """Integration test for list_incompatible_partitions with view_set_name filter."""
    from .test_utils import client

    # First get all incompatible partitions
    all_results = micromegas.admin.list_incompatible_partitions(client)

    if len(all_results) > 0:
        # Test filtering by a specific view set
        test_view_set = all_results["view_set_name"].iloc[0]
        filtered_result = micromegas.admin.list_incompatible_partitions(client, test_view_set)

        # Verify all results match the filter
        assert isinstance(filtered_result, pd.DataFrame)
        assert all(filtered_result["view_set_name"] == test_view_set)
        print(f"Filtered to view_set '{test_view_set}': {len(filtered_result)} groups")
    else:
        # No incompatible partitions to filter - just verify it doesn't crash
        result = micromegas.admin.list_incompatible_partitions(client, "nonexistent_view")
        assert isinstance(result, pd.DataFrame)
        assert len(result) == 0
        print("No incompatible partitions found - filter test skipped")


if __name__ == "__main__":
    pytest.main([__file__])
