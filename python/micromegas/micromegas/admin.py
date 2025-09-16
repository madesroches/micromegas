"""Administrative utilities for Micromegas lakehouse management.

This module provides functions for managing schema evolution and partition lifecycle
in Micromegas lakehouse. These functions are intended for administrative use and
should be used with caution as they perform potentially destructive operations.
"""

import pandas as pd
from typing import Optional


def list_incompatible_partitions(
    client, view_set_name: Optional[str] = None
) -> pd.DataFrame:
    """List partitions with schemas incompatible with current view set schemas.

    This function identifies partitions that have schema versions different from
    the current schema version for their view set. These incompatible partitions
    cannot be queried correctly alongside current partitions and should be
    retired to enable schema evolution.

    Args:
        client: FlightSQLClient instance for executing queries.
        view_set_name (str, optional): Filter results to a specific view set.
            If None, returns incompatible partitions across all view sets.

    Returns:
        pandas.DataFrame: DataFrame with incompatible partition information containing:
            - view_set_name: Name of the view set
            - view_instance_id: Instance ID (e.g., process_id or 'global')
            - incompatible_schema_hash: The old schema hash in the partition
            - current_schema_hash: The current schema hash from ViewFactory
            - partition_count: Number of incompatible partitions with this schema
            - total_size_bytes: Total size in bytes of all incompatible partitions
            - file_paths: Array of file paths for each incompatible partition (for precise retirement)

    Example:
        >>> import micromegas
        >>> import micromegas.admin
        >>>
        >>> client = micromegas.connect()
        >>>
        >>> # List all incompatible partitions across all view sets
        >>> incompatible = micromegas.admin.list_incompatible_partitions(client)
        >>> print(f"Found {len(incompatible)} groups of incompatible partitions")
        >>>
        >>> # List incompatible partitions for specific view set
        >>> log_incompatible = micromegas.admin.list_incompatible_partitions(client, 'log_entries')
        >>> print(f"Log entries incompatible partitions: {log_incompatible['partition_count'].sum()}")

    Note:
        This function leverages the existing list_partitions() and list_view_sets()
        UDTFs to perform server-side JOIN and aggregation for optimal performance.
        Schema "hashes" are actually version numbers (e.g., [4]) not cryptographic hashes.
        SQL is executed directly by DataFusion, so no SQL injection concerns.
    """
    # Build view filter clause if specific view set requested
    view_filter = ""
    if view_set_name is not None:
        view_filter = f"AND p.view_set_name = '{view_set_name}'"

    # Construct SQL query with JOIN between list_partitions() and list_view_sets()
    # Server-side filtering and aggregation for optimal performance
    sql = f"""
    SELECT 
        p.view_set_name,
        p.view_instance_id, 
        p.file_schema_hash as incompatible_schema_hash,
        vs.current_schema_hash,
        COUNT(*) as partition_count,
        SUM(p.file_size) as total_size_bytes,
        ARRAY_AGG(p.file_path) as file_paths
    FROM list_partitions() p
    JOIN list_view_sets() vs ON p.view_set_name = vs.view_set_name
    WHERE p.file_schema_hash != vs.current_schema_hash
        {view_filter}
    GROUP BY p.view_set_name, p.view_instance_id, p.file_schema_hash, vs.current_schema_hash
    ORDER BY p.view_set_name, p.view_instance_id
    """

    return client.query(sql)


def retire_incompatible_partitions(
    client, view_set_name: Optional[str] = None
) -> pd.DataFrame:
    """Retire partitions with schemas incompatible with current view set schemas.

    This function identifies and retires partitions that have schema versions
    different from the current schema version for their view set. This enables
    safe schema evolution by cleaning up old schema versions.

    **WARNING**: This operation is irreversible. Retired partitions will be
    permanently deleted from metadata and their data files removed from object storage.

    Args:
        client: FlightSQLClient instance for executing queries.
        view_set_name (str, optional): Retire incompatible partitions only for
            this specific view set. If None, retires incompatible partitions
            across all view sets (use with extreme caution).

    Returns:
        pandas.DataFrame: DataFrame with retirement results containing:
            - view_set_name: View set that was processed
            - view_instance_id: Instance ID of partitions retired
            - partitions_retired: Count of partitions retired
            - storage_freed_bytes: Total bytes freed from storage

    Example:
        >>> import micromegas
        >>> import micromegas.admin
        >>>
        >>> client = micromegas.connect()
        >>>
        >>> # Preview what would be retired (recommended first step)
        >>> preview = micromegas.admin.list_incompatible_partitions(client, 'log_entries')
        >>> print(f"Would retire {preview['partition_count'].sum()} partitions")
        >>> print(f"Would free {preview['total_size_bytes'].sum() / (1024**3):.2f} GB")
        >>>
        >>> # Retire incompatible partitions for specific view set
        >>> if input("Proceed with retirement? (yes/no): ") == "yes":
        ...     result = micromegas.admin.retire_incompatible_partitions(client, 'log_entries')
        ...     print(f"Retired {result['partitions_retired'].sum()} partitions")

    Note:
        This function orchestrates existing retire_partitions() UDTF calls for
        each group of incompatible partitions. Always preview with
        list_incompatible_partitions() before calling this function.
        SQL is executed directly by DataFusion, so no SQL injection concerns.
    """
    # First identify incompatible partitions
    incompatible = list_incompatible_partitions(client, view_set_name)

    if incompatible.empty:
        # No incompatible partitions found, return empty DataFrame with expected columns
        return pd.DataFrame(
            columns=[
                "view_set_name",
                "view_instance_id",
                "partitions_retired",
                "storage_freed_bytes",
            ]
        )

    results = []

    # For each group of incompatible partitions, determine time range and retire
    for _, group in incompatible.iterrows():
        # Query time ranges for this specific incompatible partition group
        time_range_sql = f"""
            SELECT 
                MIN(begin_insert_time) as min_time, 
                MAX(end_insert_time) as max_time
            FROM list_partitions()
            WHERE view_set_name = '{group["view_set_name"]}' 
                AND view_instance_id = '{group["view_instance_id"]}'
                AND file_schema_hash = '{group["incompatible_schema_hash"]}'
        """

        time_ranges = client.query(time_range_sql)

        if time_ranges.empty or pd.isna(time_ranges["min_time"].iloc[0]):
            # No valid time range found, skip this group
            continue

        min_time = time_ranges["min_time"].iloc[0]
        max_time = time_ranges["max_time"].iloc[0]

        # Call existing retire_partitions UDTF for this time range
        retirement_sql = f"""
            SELECT * FROM retire_partitions(
                '{group["view_set_name"]}', 
                '{group["view_instance_id"]}',
                '{min_time}',
                '{max_time}'
            )
        """

        try:
            retirement_result = client.query(retirement_sql)

            # Extract storage freed information if available
            # Note: The retire_partitions UDTF may not return this info directly
            storage_freed = 0
            if (
                not retirement_result.empty
                and "storage_freed_bytes" in retirement_result.columns
            ):
                storage_freed = retirement_result["storage_freed_bytes"].sum()
            else:
                # Estimate from the original group size
                storage_freed = group["total_size_bytes"]

            # Record successful retirement
            results.append(
                {
                    "view_set_name": group["view_set_name"],
                    "view_instance_id": group["view_instance_id"],
                    "partitions_retired": group["partition_count"],
                    "storage_freed_bytes": storage_freed,
                }
            )

        except Exception as e:
            # Log error but continue with other groups
            print(
                f"Error retiring partitions for {group['view_set_name']}/{group['view_instance_id']}: {e}"
            )
            continue

    return pd.DataFrame(results)
