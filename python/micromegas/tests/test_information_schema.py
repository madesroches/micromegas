#!/usr/bin/python3
"""Tests for DataFusion information_schema support (SHOW TABLES, etc.)"""
from .test_utils import client


def test_show_tables():
    """Test that SHOW TABLES returns results."""
    df = client.query("SHOW TABLES")
    print("\nSHOW TABLES result:")
    print(df)

    # Verify the expected columns are present
    assert "table_catalog" in df.columns
    assert "table_schema" in df.columns
    assert "table_name" in df.columns
    assert "table_type" in df.columns

    # Verify we have at least some tables (the registered views)
    assert len(df) > 0, "SHOW TABLES should return at least one table"

    # Check that our known tables are listed
    table_names = df["table_name"].tolist()
    print(f"\nFound {len(table_names)} tables: {table_names}")


def test_show_tables_contains_known_views():
    """Test that SHOW TABLES includes known micromegas views."""
    df = client.query("SHOW TABLES")
    table_names = df["table_name"].tolist()

    # These are global views that should be registered
    expected_tables = ["log_entries", "blocks", "streams", "processes"]

    for expected in expected_tables:
        assert (
            expected in table_names
        ), f"Expected table '{expected}' not found in SHOW TABLES"


def test_information_schema_tables():
    """Test querying information_schema.tables directly."""
    df = client.query("SELECT * FROM information_schema.tables")
    print("\ninformation_schema.tables result:")
    print(df)

    assert len(df) > 0, "information_schema.tables should have entries"
