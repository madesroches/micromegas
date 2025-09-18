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
    are ignored during queries but take up storage space and should be
    retired to free storage and enable clean schema evolution.

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

    **SAFETY**: This function retires only the exact incompatible partitions by
    their file paths, ensuring no compatible partitions are accidentally retired.

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
            - partitions_retired: Count of partitions successfully retired
            - partitions_failed: Count of partitions that failed to retire
            - storage_freed_bytes: Total bytes freed from storage
            - retirement_messages: Array of detailed messages for each retirement attempt

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
        ...     print(f"Failed {result['partitions_failed'].sum()} partitions")

    Note:
        This function uses the retire_partition_by_file() UDF to retire each
        partition individually by its exact file path. This ensures precise
        targeting and eliminates the risk of accidentally retiring compatible
        partitions that happen to exist in the same time ranges.
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
                "partitions_failed",
                "storage_freed_bytes",
                "retirement_messages",
            ]
        )

    results = []

    # For each group of incompatible partitions, retire by individual file paths
    for _, group in incompatible.iterrows():
        file_paths = group["file_paths"]

        # Convert file_paths to list if it's not already (handle different pandas array types)
        if hasattr(file_paths, "tolist"):
            file_paths_list = file_paths.tolist()
        elif isinstance(file_paths, str):
            # Single file path case
            file_paths_list = [file_paths]
        else:
            file_paths_list = list(file_paths)

        retirement_messages = []
        partitions_retired = 0
        partitions_failed = 0

        # Retire each partition individually using the targeted UDF
        for file_path in file_paths_list:
            if not file_path or pd.isna(file_path):
                continue

            try:
                # Use the new retire_partition_by_file UDF
                retirement_sql = (
                    f"SELECT retire_partition_by_file('{file_path}') as message"
                )
                retirement_result = client.query(retirement_sql)

                if not retirement_result.empty:
                    message = retirement_result["message"].iloc[0]
                    retirement_messages.append(message)

                    if message.startswith("SUCCESS:"):
                        partitions_retired += 1
                    else:
                        partitions_failed += 1
                        print(f"Warning: Failed to retire {file_path}: {message}")
                else:
                    partitions_failed += 1
                    retirement_messages.append(
                        f"ERROR: No result returned for {file_path}"
                    )

            except Exception as e:
                partitions_failed += 1
                error_msg = f"ERROR: Exception retiring {file_path}: {e}"
                retirement_messages.append(error_msg)
                print(f"Error retiring partition {file_path}: {e}")

        # Calculate storage freed (only count successful retirements)
        if partitions_retired > 0 and group["partition_count"] > 0:
            # Proportional calculation based on successful retirements
            storage_freed = int(
                group["total_size_bytes"]
                * (partitions_retired / group["partition_count"])
            )
        else:
            storage_freed = 0

        # Record retirement results for this group
        results.append(
            {
                "view_set_name": group["view_set_name"],
                "view_instance_id": group["view_instance_id"],
                "partitions_retired": partitions_retired,
                "partitions_failed": partitions_failed,
                "storage_freed_bytes": storage_freed,
                "retirement_messages": retirement_messages,
            }
        )

    return pd.DataFrame(results)
