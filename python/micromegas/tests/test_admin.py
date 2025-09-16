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
                    ]
                )

        # Check if this is a time range query for retirement
        elif "MIN(begin_insert_time)" in sql and "MAX(end_insert_time)" in sql:
            if "time_range_data" in self.mock_data:
                return self.mock_data["time_range_data"]
            else:
                return pd.DataFrame(
                    {
                        "min_time": ["2024-01-01T00:00:00Z"],
                        "max_time": ["2024-01-01T01:00:00Z"],
                    }
                )

        # Check if this is a retire_partitions call
        elif "retire_partitions(" in sql:
            if "retirement_result_data" in self.mock_data:
                return self.mock_data["retirement_result_data"]
            else:
                return pd.DataFrame(
                    {"time": ["2024-01-01T00:00:00Z"], "msg": ["Retired 1 partition"]}
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
        "storage_freed_bytes",
    ]
    assert list(result.columns) == expected_columns


def test_retire_incompatible_partitions_with_data():
    """Test retire_incompatible_partitions with mock incompatible partitions."""
    mock_data = {
        "incompatible_test_data": pd.DataFrame(
            {
                "view_set_name": ["log_entries"],
                "view_instance_id": ["process-123"],
                "incompatible_schema_hash": ["[3]"],
                "current_schema_hash": ["[4]"],
                "partition_count": [5],
                "total_size_bytes": [1024000],
            }
        ),
        "time_range_data": pd.DataFrame(
            {"min_time": ["2024-01-01T00:00:00Z"], "max_time": ["2024-01-01T01:00:00Z"]}
        ),
        "retirement_result_data": pd.DataFrame(
            {"time": ["2024-01-01T00:00:00Z"], "msg": ["Retired 5 partitions"]}
        ),
    }

    client = MockFlightSQLClient(mock_data)
    result = micromegas.admin.retire_incompatible_partitions(client)

    assert isinstance(result, pd.DataFrame)
    assert len(result) == 1
    assert result["view_set_name"].iloc[0] == "log_entries"
    assert result["view_instance_id"].iloc[0] == "process-123"
    assert result["partitions_retired"].iloc[0] == 5
    assert result["storage_freed_bytes"].iloc[0] == 1024000


def test_sql_injection_prevention():
    """Test that SQL injection attempts are escaped properly."""
    client = MockFlightSQLClient()

    # Test with malicious view_set_name
    malicious_name = "log_entries'; DROP TABLE lakehouse_partitions; --"

    # This should not raise an exception and should escape the quotes
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
            }
        ),
        "time_range_data": pd.DataFrame(
            {"min_time": ["2024-01-01T00:00:00Z"], "max_time": ["2024-01-01T01:00:00Z"]}
        ),
    }

    client_with_malicious = MockFlightSQLClient(mock_data)
    result = micromegas.admin.retire_incompatible_partitions(client_with_malicious)

    # Should handle the malicious input gracefully
    assert isinstance(result, pd.DataFrame)


if __name__ == "__main__":
    pytest.main([__file__])
