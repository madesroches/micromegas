"""Administrative utilities for Micromegas lakehouse management.

This module provides functions for managing schema evolution and partition lifecycle
in Micromegas lakehouse. These functions are intended for administrative use and
should be used with caution as they perform potentially destructive operations.
"""

import pandas as pd
from typing import Optional

from micromegas.time import format_datetime


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
            - begin_insert_time: Begin insert time of the partition
            - end_insert_time: End insert time of the partition
            - incompatible_schema_hash: The old schema hash in the partition
            - current_schema_hash: The current schema hash from ViewFactory
            - file_path: File path for the partition (NULL for empty partitions)
            - file_size: Size in bytes of the partition file (0 for empty partitions)

    Example:
        >>> import micromegas
        >>> import micromegas.admin
        >>>
        >>> client = micromegas.connect()
        >>>
        >>> # List all incompatible partitions across all view sets
        >>> incompatible = micromegas.admin.list_incompatible_partitions(client)
        >>> print(f"Found {len(incompatible)} incompatible partitions")
        >>>
        >>> # List incompatible partitions for specific view set
        >>> log_incompatible = micromegas.admin.list_incompatible_partitions(client, 'log_entries')
        >>> print(f"Log entries incompatible partitions: {len(log_incompatible)}")

    Note:
        This function leverages the existing list_partitions() and list_view_sets()
        UDTFs to perform server-side JOIN for optimal performance.
        Schema "hashes" are actually version numbers (e.g., [4]) not cryptographic hashes.
        SQL is executed directly by DataFusion, so no SQL injection concerns.
    """
    # Build view filter clause if specific view set requested
    view_filter = ""
    if view_set_name is not None:
        view_filter = f"AND p.view_set_name = '{view_set_name}'"

    # Construct SQL query with JOIN between list_partitions() and list_view_sets()
    # Return one row per partition (no aggregation) for metadata-based retirement
    sql = f"""
    SELECT 
        p.view_set_name,
        p.view_instance_id,
        p.begin_insert_time,
        p.end_insert_time,
        p.file_schema_hash as incompatible_schema_hash,
        vs.current_schema_hash,
        p.file_path,
        p.file_size
    FROM list_partitions() p
    JOIN list_view_sets() vs ON p.view_set_name = vs.view_set_name
    WHERE p.file_schema_hash != vs.current_schema_hash
        {view_filter}
    ORDER BY p.view_set_name, p.view_instance_id, p.begin_insert_time
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
    their metadata identifiers, ensuring no compatible partitions are accidentally retired.

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
        >>> print(f"Would retire {len(preview)} partitions")
        >>> print(f"Would free {preview['file_size'].sum() / (1024**3):.2f} GB")
        >>>
        >>> # Retire incompatible partitions for specific view set
        >>> if input("Proceed with retirement? (yes/no): ") == "yes":
        ...     result = micromegas.admin.retire_incompatible_partitions(client, 'log_entries')
        ...     print(f"Retired {result['partitions_retired'].sum()} partitions")
        ...     print(f"Failed {result['partitions_failed'].sum()} partitions")

    Note:
        This function uses the retire_partition_by_metadata() UDF to retire each
        partition individually by its metadata identifiers (view_set_name, view_instance_id,
        begin_insert_time, end_insert_time). This works for both empty partitions
        (file_path=NULL) and non-empty partitions, ensuring complete cleanup.
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

    # Group by view_set_name and view_instance_id for aggregated results
    results = []
    for (view_set, view_instance), group in incompatible.groupby(
        ["view_set_name", "view_instance_id"]
    ):
        retirement_messages = []
        partitions_retired = 0
        partitions_failed = 0
        storage_freed = 0

        # Retire each partition individually using metadata-based retirement
        for _, partition in group.iterrows():
            try:
                # Use retire_partition_by_metadata UDF
                begin_time = format_datetime(partition["begin_insert_time"])
                end_time = format_datetime(partition["end_insert_time"])
                retirement_sql = f"""
                SELECT retire_partition_by_metadata(
                    '{partition['view_set_name']}',
                    '{partition['view_instance_id']}',
                    CAST('{begin_time}' AS TIMESTAMP),
                    CAST('{end_time}' AS TIMESTAMP)
                ) as message
                """
                retirement_result = client.query(retirement_sql)

                if not retirement_result.empty:
                    message = retirement_result["message"].iloc[0]
                    retirement_messages.append(message)

                    if message.startswith("SUCCESS:"):
                        partitions_retired += 1
                        storage_freed += int(partition["file_size"])
                    else:
                        partitions_failed += 1
                        print(
                            f"Warning: Failed to retire partition {partition['view_set_name']}/"
                            f"{partition['view_instance_id']} [{partition['begin_insert_time']}, "
                            f"{partition['end_insert_time']}): {message}"
                        )
                else:
                    partitions_failed += 1
                    retirement_messages.append(
                        f"ERROR: No result returned for partition {partition['view_set_name']}/"
                        f"{partition['view_instance_id']} [{partition['begin_insert_time']}, "
                        f"{partition['end_insert_time']})"
                    )

            except Exception as e:
                partitions_failed += 1
                error_msg = (
                    f"ERROR: Exception retiring partition {partition['view_set_name']}/"
                    f"{partition['view_instance_id']} [{partition['begin_insert_time']}, "
                    f"{partition['end_insert_time']}): {e}"
                )
                retirement_messages.append(error_msg)
                print(error_msg)

        # Record retirement results for this group
        results.append(
            {
                "view_set_name": view_set,
                "view_instance_id": view_instance,
                "partitions_retired": partitions_retired,
                "partitions_failed": partitions_failed,
                "storage_freed_bytes": storage_freed,
                "retirement_messages": retirement_messages,
            }
        )

    return pd.DataFrame(results)
