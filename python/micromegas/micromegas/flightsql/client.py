from . import FlightSql_pb2
from . import time
from google.protobuf import any_pb2
from pyarrow import flight
from typing import Any, Optional, Callable
import certifi
import pyarrow
import sys
import warnings


class MicromegasMiddleware(flight.ClientMiddleware):
    def __init__(self, headers):
        self.headers = headers

    def call_completed(self, exception):
        if exception is not None:
            print(exception, file=sys.stderr)

    def received_headers(self, headers):
        pass

    def sending_headers(self):
        return self.headers


class MicromegasMiddlewareFactory(flight.ClientMiddlewareFactory):
    def __init__(self, headers):
        self.headers = headers

    def start_call(self, info):
        return MicromegasMiddleware(self.headers)


class DynamicAuthMiddleware(flight.ClientMiddleware):
    """Middleware that dynamically fetches auth tokens for each request."""

    def __init__(self, auth_provider):
        self.auth_provider = auth_provider

    def call_completed(self, exception):
        if exception is not None:
            print(exception, file=sys.stderr)

    def received_headers(self, headers):
        pass

    def sending_headers(self):
        """Get fresh auth token for each request."""
        token = self.auth_provider.get_token()
        return {"authorization": f"Bearer {token}".encode("utf8")}


class DynamicAuthMiddlewareFactory(flight.ClientMiddlewareFactory):
    """Factory for creating dynamic auth middleware."""

    def __init__(self, auth_provider):
        self.auth_provider = auth_provider

    def start_call(self, info):
        return DynamicAuthMiddleware(self.auth_provider)


def make_call_headers(begin, end, preserve_dictionary=False):
    call_headers = [
        ("x-client-type".encode("utf8"), "python".encode("utf8")),
    ]
    if begin is not None:
        call_headers.append(
            (
                "query_range_begin".encode("utf8"),
                time.format_datetime(begin).encode("utf8"),
            )
        )
    if end is not None:
        call_headers.append(
            (
                "query_range_end".encode("utf8"),
                time.format_datetime(end).encode("utf8"),
            )
        )
    if preserve_dictionary:
        call_headers.append(
            (
                "preserve_dictionary".encode("utf8"),
                "true".encode("utf8"),
            )
        )
    return call_headers


def make_prepared_statement_action(sql):
    request = FlightSql_pb2.ActionCreatePreparedStatementRequest(query=sql)
    any = any_pb2.Any()
    any.Pack(request)
    action_type = "CreatePreparedStatement"
    return flight.Action(action_type, any.SerializeToString())


def make_query_ticket(sql):
    ticket_statement_query = FlightSql_pb2.TicketStatementQuery(
        statement_handle=sql.encode("utf8")
    )
    any = any_pb2.Any()
    any.Pack(ticket_statement_query)
    ticket = flight.Ticket(any.SerializeToString())
    return ticket


def make_arrow_flight_descriptor(command: Any) -> flight.FlightDescriptor:
    any = any_pb2.Any()
    any.Pack(command)
    return flight.FlightDescriptor.for_command(any.SerializeToString())


def make_ingest_flight_desc(table_name):
    ingest_statement = FlightSql_pb2.CommandStatementIngest(
        table=table_name, temporary=False
    )
    desc = make_arrow_flight_descriptor(ingest_statement)
    return desc


class PreparedStatement:
    """Represents a prepared SQL statement with its result schema.

    Prepared statements in Micromegas are primarily used for schema discovery -
    determining the structure of query results without executing the query.
    This is useful for query validation and building dynamic interfaces.

    Attributes:
        query (str): The SQL query string for this prepared statement.
        dataset_schema (pyarrow.Schema): The schema (column names and types) of the result set.

    Example:
        >>> stmt = client.prepare_statement("SELECT time, level, msg FROM log_entries")
        >>> print(stmt.query)
        >>> # Output: "SELECT time, level, msg FROM log_entries"
        >>>
        >>> # Inspect the schema without running the query
        >>> for field in stmt.dataset_schema:
        ...     print(f"{field.name}: {field.type}")
        >>> # Output: time: timestamp[ns]
        >>> #         level: int32
        >>> #         msg: string
    """

    def __init__(self, prepared_statement_result):
        """Initialize a PreparedStatement from server response.

        Args:
            prepared_statement_result: The server's response containing the prepared
                statement handle and dataset schema.
        """
        self.query = prepared_statement_result.prepared_statement_handle.decode("utf8")
        reader = pyarrow.ipc.open_stream(prepared_statement_result.dataset_schema)
        self.dataset_schema = reader.schema
        reader.close()


class FlightSQLClient:
    """Client for querying Micromegas observability data using Apache Arrow FlightSQL.

    This client provides high-performance access to telemetry data stored in Micromegas,
    supporting both simple queries and advanced features like prepared statements,
    bulk ingestion, and partition management.

    The client uses Apache Arrow's columnar format for efficient data transfer and
    supports streaming for large result sets.
    """

    def __init__(
        self, uri, headers=None, preserve_dictionary=False, auth_provider=None
    ):
        """Initialize a FlightSQL client connection.

        Args:
            uri (str): The FlightSQL server URI (e.g., "grpc://localhost:50051").
                Use "grpc://" for unencrypted connections or "grpc+tls://" for TLS.
            headers (dict, optional): **Deprecated.** Use auth_provider instead.
                Static headers for authentication. This parameter is deprecated because
                it doesn't support automatic token refresh.
            preserve_dictionary (bool, optional): When True, preserve dictionary encoding in
                Arrow arrays for memory efficiency. Useful when using dictionary-encoded UDFs.
                Defaults to False for backward compatibility.
            auth_provider (optional): Authentication provider that implements get_token() method.
                When provided, tokens are automatically refreshed before each request.
                Example: OidcAuthProvider. This is the recommended way to handle authentication.

        Example:
            >>> # Connect to local server
            >>> client = FlightSQLClient("grpc://localhost:50051")
            >>>
            >>> # Connect with OIDC authentication (recommended - automatic token refresh)
            >>> from micromegas.auth import OidcAuthProvider
            >>> auth = OidcAuthProvider.from_file("~/.micromegas/tokens.json")
            >>> client = FlightSQLClient(
            ...     "grpc+tls://remote-server:50051",
            ...     auth_provider=auth
            ... )
            >>>
            >>> # Connect with dictionary preservation for memory efficiency
            >>> client = FlightSQLClient(
            ...     "grpc://localhost:50051",
            ...     preserve_dictionary=True
            ... )
        """
        # Emit deprecation warning if headers is used
        if headers is not None:
            warnings.warn(
                "The 'headers' parameter is deprecated and will be removed in a future version. "
                "Use 'auth_provider' parameter instead for automatic token refresh support.",
                DeprecationWarning,
                stacklevel=2,
            )

        # Normalize URI scheme for Arrow Flight
        uri = self._normalize_uri(uri)

        fh = open(certifi.where(), "r")
        cert = fh.read()
        fh.close()

        # Choose middleware based on auth_provider or static headers
        if auth_provider is not None:
            factory = DynamicAuthMiddlewareFactory(auth_provider)
        else:
            factory = MicromegasMiddlewareFactory(headers)

        self.__flight_client = flight.connect(
            location=uri, tls_root_certs=cert, middleware=[factory]
        )
        self.__preserve_dictionary = preserve_dictionary

    @staticmethod
    def _normalize_uri(uri: str) -> str:
        """Normalize URI scheme for Arrow Flight.

        Arrow Flight uses gRPC protocols, not HTTP. This method converts
        common HTTP-style URIs to the correct gRPC schemes:
        - https:// -> grpc+tls://
        - http:// -> grpc://
        """
        if uri.startswith("https://"):
            return "grpc+tls://" + uri[8:]
        elif uri.startswith("http://"):
            return "grpc://" + uri[7:]
        return uri

    def _prepare_table_for_pandas(self, table):
        """Prepare Arrow table with dictionary columns for pandas conversion.

        As of PyArrow/pandas 2024-2025, dictionary-encoded complex types
        (List, Struct, Union) cannot be converted directly to pandas due to
        "ArrowNotImplementedError: Unification of ... dictionaries is not implemented".

        This method converts problematic dictionary columns back to regular arrays
        while preserving memory efficiency during Arrow processing.
        """
        import pyarrow.compute as pc

        columns = []
        column_names = []

        for i, column in enumerate(table.columns):
            column_name = table.column_names[i]
            column_names.append(column_name)

            # Check if this is a dictionary-encoded column
            if pyarrow.types.is_dictionary(column.type):
                value_type = column.type.value_type

                # Convert dictionary-encoded complex types that pandas can't handle
                if (
                    pyarrow.types.is_list(value_type)
                    or pyarrow.types.is_struct(value_type)
                    or pyarrow.types.is_union(value_type)
                ):
                    # Manually decode dictionary by reconstructing the array
                    # This works around PyArrow's casting limitations

                    # Decode each chunk of the dictionary column
                    reconstructed_chunks = []

                    if hasattr(column, "chunks"):
                        # ChunkedArray case
                        for chunk in column.chunks:
                            indices = chunk.indices
                            dictionary = chunk.dictionary
                            reconstructed_chunk = pc.take(dictionary, indices)
                            reconstructed_chunks.append(reconstructed_chunk)

                        # Create a new ChunkedArray from reconstructed chunks
                        reconstructed = pyarrow.chunked_array(reconstructed_chunks)
                    else:
                        # Single Array case
                        indices = column.indices
                        dictionary = column.dictionary
                        reconstructed = pc.take(dictionary, indices)

                    columns.append(reconstructed)
                else:
                    # Keep simple dictionary types (strings, numbers) for pandas
                    # These work fine and provide memory benefits in pandas too
                    columns.append(column)
            else:
                # Non-dictionary columns are fine as-is
                columns.append(column)

        return pyarrow.Table.from_arrays(columns, names=column_names)

    def query(self, sql, begin=None, end=None):
        """Execute a SQL query and return results as a pandas DataFrame.

        This method executes the provided SQL query and returns all results in memory
        as a pandas DataFrame. For large result sets, consider using query_stream() instead.

        Args:
            sql (str): The SQL query to execute. Can use any supported SQL syntax including
                JOINs, aggregations, window functions, etc.
            begin (datetime or str, optional): Start time for partition pruning. Significantly
                improves performance by eliminating irrelevant partitions before query execution.
                Can be a timezone-aware datetime or RFC3339 string (e.g., "2024-01-01T00:00:00Z").
            end (datetime or str, optional): End time for partition pruning. Should be used
                together with begin for optimal performance.

        Returns:
            pandas.DataFrame: Query results with appropriate column types. When the client was
                created with preserve_dictionary=True, dictionary-encoded columns will maintain
                their encoding for memory efficiency.

        Raises:
            Exception: If the query fails due to syntax errors, missing tables, or server issues.

        Example:
            >>> import datetime
            >>>
            >>> # Query with time range for optimal performance
            >>> end = datetime.datetime.now(datetime.timezone.utc)
            >>> begin = end - datetime.timedelta(hours=1)
            >>> df = client.query(
            ...     "SELECT time, level, msg FROM log_entries WHERE level <= 3",
            ...     begin, end
            ... )
            >>>
            >>> # For dictionary preservation, create client with preserve_dictionary=True
            >>> dict_client = FlightSQLClient("grpc://localhost:50051", preserve_dictionary=True)
            >>> df = dict_client.query("SELECT dict_encoded_column FROM table")

        Performance Note:
            Always provide begin/end parameters when querying time-series data to enable
            partition pruning, which can improve query performance by 10-100x.
            Use preserve_dictionary=True in client constructor with dictionary-encoded UDFs
            for significant memory reduction.
        """
        call_headers = make_call_headers(begin, end, self.__preserve_dictionary)
        options = flight.FlightCallOptions(headers=call_headers)
        ticket = make_query_ticket(sql)
        reader = self.__flight_client.do_get(ticket, options=options)
        record_batches = []
        for chunk in reader:
            record_batches.append(chunk.data)
        table = pyarrow.Table.from_batches(record_batches, reader.schema)

        # Handle dictionary-encoded columns that pandas can't convert directly
        if self.__preserve_dictionary:
            table = self._prepare_table_for_pandas(table)

        return table.to_pandas()

    def query_stream(self, sql, begin=None, end=None):
        """Execute a SQL query and stream results as Arrow RecordBatch objects.

        This method is ideal for large result sets as it processes data in chunks,
        avoiding memory issues and allowing processing to start before the query completes.

        Args:
            sql (str): The SQL query to execute.
            begin (datetime or str, optional): Start time for partition pruning.
                Can be a timezone-aware datetime or RFC3339 string.
            end (datetime or str, optional): End time for partition pruning.

        Yields:
            pyarrow.RecordBatch: Chunks of query results. Each batch contains a subset
                of rows with all columns from the query. When the client was created with
                preserve_dictionary=True, dictionary-encoded columns will maintain their encoding.

        Example:
            >>> # Stream and process large dataset
            >>> total_errors = 0
            >>> for batch in client.query_stream(
            ...     "SELECT * FROM log_entries WHERE level <= 2",
            ...     begin, end
            ... ):
            ...     df_chunk = batch.to_pandas()
            ...     total_errors += len(df_chunk)
            ...     # Process chunk and release memory
            ... print(f"Total errors: {total_errors}")
            >>>
            >>> # Stream with dictionary preservation
            >>> dict_client = FlightSQLClient("grpc://localhost:50051", preserve_dictionary=True)
            >>> for batch in dict_client.query_stream("SELECT dict_encoded_column FROM table"):
            ...     # Process dictionary-encoded data efficiently
            ...     pass

        Performance Note:
            Streaming is recommended when:
            - Result set is larger than 100MB
            - You want to start processing before the query completes
            - Memory usage needs to be controlled
            Use preserve_dictionary=True in client constructor with dictionary-encoded UDFs
            for significant memory reduction.
        """
        ticket = make_query_ticket(sql)
        call_headers = make_call_headers(begin, end, self.__preserve_dictionary)
        options = flight.FlightCallOptions(headers=call_headers)
        reader = self.__flight_client.do_get(ticket, options=options)
        record_batches = []
        for chunk in reader:
            yield chunk.data

    def query_arrow(self, sql, begin=None, end=None):
        """Execute a SQL query and return results as an Arrow Table.

        This method preserves dictionary encoding and avoids pandas conversion issues.
        Useful for working directly with Arrow data or when pandas can't handle
        dictionary-encoded complex types.

        Args:
            sql (str): The SQL query to execute.
            begin (datetime or str, optional): Start time for partition pruning.
            end (datetime or str, optional): End time for partition pruning.

        Returns:
            pyarrow.Table: Query results as Arrow Table with preserved dictionary encoding.

        Example:
            >>> # Get Arrow table with preserved dictionary encoding
            >>> table = client.query_arrow("SELECT dict_encoded_column FROM table")
            >>> print(table.schema)  # Shows dictionary<...> types
            >>>
            >>> # Work with Arrow directly to avoid pandas limitations
            >>> for batch in table.to_batches():
            ...     # Process Arrow data without pandas conversion
            ...     pass
        """
        call_headers = make_call_headers(begin, end, self.__preserve_dictionary)
        options = flight.FlightCallOptions(headers=call_headers)
        ticket = make_query_ticket(sql)
        reader = self.__flight_client.do_get(ticket, options=options)
        record_batches = []
        for chunk in reader:
            record_batches.append(chunk.data)
        return pyarrow.Table.from_batches(record_batches, reader.schema)

    def prepare_statement(self, sql):
        """Create a prepared statement to retrieve query schema without executing it.

        Prepared statements in Micromegas are primarily used to determine the schema
        (column names and types) of a query result without actually executing the query
        and retrieving data. This is useful for validating queries or building dynamic
        interfaces that need to know the result structure in advance.

        Args:
            sql (str): The SQL query to prepare and analyze.

        Returns:
            PreparedStatement: An object containing the query and its result schema.

        Example:
            >>> # Get schema information without executing the query
            >>> stmt = client.prepare_statement(
            ...     "SELECT time, level, msg FROM log_entries WHERE level <= 3"
            ... )
            >>>
            >>> # Access the schema
            >>> print(stmt.dataset_schema)
            >>> # Output: time: timestamp[ns]
            >>> #         level: int32
            >>> #         msg: string
            >>>
            >>> # The query text is also available
            >>> print(stmt.query)
            >>> # Output: "SELECT time, level, msg FROM log_entries WHERE level <= 3"

        Note:
            The primary purpose is schema discovery. The prepared statement can be
            executed via prepared_statement_stream(), but this offers no performance
            benefit over regular query_stream() in the current implementation.
        """
        action = make_prepared_statement_action(sql)
        results = self.__flight_client.do_action(action)
        for result in list(results):
            any = any_pb2.Any()
            any.ParseFromString(result.body.to_pybytes())
            res = FlightSql_pb2.ActionCreatePreparedStatementResult()
            any.Unpack(res)
            return PreparedStatement(res)

    def prepared_statement_stream(self, statement):
        """Execute a prepared statement and stream results.

        Executes a previously prepared statement and returns results as a stream of
        Arrow RecordBatch objects. This is functionally equivalent to calling
        query_stream() with the statement's SQL query.

        Args:
            statement (PreparedStatement): The prepared statement to execute,
                obtained from prepare_statement().

        Yields:
            pyarrow.RecordBatch: Chunks of query results.

        Example:
            >>> # Prepare statement (mainly for schema discovery)
            >>> stmt = client.prepare_statement("SELECT time, level, msg FROM log_entries")
            >>>
            >>> # Check schema before execution
            >>> print(f"Query will return {len(stmt.dataset_schema)} columns")
            >>>
            >>> # Execute the prepared statement
            >>> for batch in client.prepared_statement_stream(stmt):
            ...     df = batch.to_pandas()
            ...     print(f"Received batch with {len(df)} rows")

        Note:
            This offers no performance advantage over query_stream(statement.query).
            The main benefit of prepared statements is schema discovery via prepare_statement().
        """
        # because we are not serializing the logical plan in the prepared statement, we can just execute the query normally
        return self.query_stream(statement.query)

    def bulk_ingest(self, table_name, df):
        """Bulk ingest a pandas DataFrame into a Micromegas metadata table.

        This method efficiently loads metadata or replication data into Micromegas
        tables using Arrow's columnar format. Primarily used for ingesting:
        - processes: Process metadata and information
        - streams: Event stream metadata
        - blocks: Data block metadata
        - payloads: Raw binary telemetry payloads (for replication)

        Args:
            table_name (str): The name of the target table. Supported tables:
                'processes', 'streams', 'blocks', 'payloads'.
            df (pandas.DataFrame): The DataFrame to ingest. Column names and types
                must exactly match the target table schema.

        Returns:
            DoPutUpdateResult or None: Server response containing ingestion statistics
                such as number of records ingested, or None if no response.

        Raises:
            Exception: If ingestion fails due to schema mismatch, unsupported table,
                or invalid data.

        Example:
            >>> import pandas as pd
            >>> from datetime import datetime, timezone
            >>>
            >>> # Example: Replicate process metadata
            >>> processes_df = pd.DataFrame({
            ...     'process_id': ['550e8400-e29b-41d4-a716-446655440000'],
            ...     'exe': ['/usr/bin/myapp'],
            ...     'username': ['user'],
            ...     'realname': ['User Name'],
            ...     'computer': ['hostname'],
            ...     'distro': ['Ubuntu 22.04'],
            ...     'cpu_brand': ['Intel Core i7'],
            ...     'tsc_frequency': [2400000000],
            ...     'start_time': [datetime.now(timezone.utc)],
            ...     'start_ticks': [1234567890],
            ...     'insert_time': [datetime.now(timezone.utc)],
            ...     'parent_process_id': [''],
            ...     'properties': [[]]
            ... })
            >>>
            >>> # Bulk ingest process metadata
            >>> result = client.bulk_ingest('processes', processes_df)
            >>> if result:
            ...     print(f"Ingested {result.record_count} process records")

        Note:
            This method is primarily intended for metadata replication and
            administrative tasks. For normal telemetry data ingestion, use
            the telemetry ingestion service HTTP API instead.
        """
        desc = make_ingest_flight_desc(table_name)
        table = pyarrow.Table.from_pandas(df)
        writer, reader = self.__flight_client.do_put(desc, table.schema)
        for rb in table.to_batches():
            writer.write(rb)
        writer.done_writing()
        result = reader.read()
        if result is not None:
            update_result = FlightSql_pb2.DoPutUpdateResult()
            update_result.ParseFromString(result.to_pybytes())
            return update_result
        else:
            return None

    def retire_partitions(self, view_set_name, view_instance_id, begin, end):
        """Remove materialized view partitions for a specific time range.

        This method removes previously materialized partitions, which is useful for:
        - Freeing up storage space
        - Removing outdated materialized data
        - Preparing for re-materialization with different settings

        Args:
            view_set_name (str): The name of the view set containing the partitions.
            view_instance_id (str): The specific view instance identifier (usually a process_id).
            begin (datetime): Start time of partitions to retire (inclusive).
            end (datetime): End time of partitions to retire (exclusive).

        Returns:
            None: Prints status messages as partitions are retired.

        Example:
            >>> from datetime import datetime, timedelta, timezone
            >>>
            >>> # Retire partitions for the last 7 days
            >>> end = datetime.now(timezone.utc)
            >>> begin = end - timedelta(days=7)
            >>>
            >>> client.retire_partitions(
            ...     'log_entries',
            ...     'process-123-456',
            ...     begin,
            ...     end
            ... )
            # Output: Timestamps and status messages for each retired partition

        Note:
            This operation cannot be undone. Retired partitions must be re-materialized
            if the data is needed again.
        """
        sql = """
          SELECT time, msg
          FROM retire_partitions('{view_set_name}', '{view_instance_id}', '{begin}', '{end}')
        """.format(
            view_set_name=view_set_name,
            view_instance_id=view_instance_id,
            begin=begin.isoformat(),
            end=end.isoformat(),
        )
        for rb in self.query_stream(sql):
            for index, row in rb.to_pandas().iterrows():
                print(row["time"], row["msg"])

    def materialize_partitions(
        self, view_set_name, begin, end, partition_delta_seconds
    ):
        """Create materialized view partitions for faster query performance.

        Materialized partitions pre-compute and store query results in optimized format,
        significantly improving query performance for frequently accessed data.

        Args:
            view_set_name (str): The name of the view set to materialize.
            begin (datetime): Start time for materialization (inclusive).
            end (datetime): End time for materialization (exclusive).
            partition_delta_seconds (int): Size of each partition in seconds.
                Common values: 3600 (hourly), 86400 (daily).

        Returns:
            None: Prints status messages as partitions are created.

        Example:
            >>> from datetime import datetime, timedelta, timezone
            >>>
            >>> # Materialize hourly partitions for the last 24 hours
            >>> end = datetime.now(timezone.utc)
            >>> begin = end - timedelta(days=1)
            >>>
            >>> client.materialize_partitions(
            ...     'log_entries',
            ...     begin,
            ...     end,
            ...     3600  # 1-hour partitions
            ... )
            # Output: Progress messages for each materialized partition

        Performance Note:
            Materialized partitions can improve query performance by 10-100x but
            require additional storage. Choose partition size based on query patterns:
            - Hourly (3600): For high-frequency queries on recent data
            - Daily (86400): For historical analysis and reporting
        """
        sql = """
          SELECT time, msg
          FROM materialize_partitions('{view_set_name}', '{begin}', '{end}', {partition_delta_seconds})
        """.format(
            view_set_name=view_set_name,
            begin=begin.isoformat(),
            end=end.isoformat(),
            partition_delta_seconds=partition_delta_seconds,
        )
        for rb in self.query_stream(sql):
            for index, row in rb.to_pandas().iterrows():
                print(row["time"], row["msg"])

    def find_process(self, process_id):
        """Find and retrieve metadata for a specific process.

        Queries the processes table to get detailed information about a process
        including its executable path, command line arguments, start time, and metadata.

        Args:
            process_id (str): The unique identifier of the process to find.
                This is typically a UUID string.

        Returns:
            pandas.DataFrame: A DataFrame containing process information with columns:
                - process_id: Unique process identifier
                - exe: Executable path
                - username: User who started the process
                - realname: Real name of the user
                - computer: Hostname where process is running
                - start_time: When the process started
                - parent_process_id: Parent process identifier
                - metadata: Additional process metadata as JSON

        Example:
            >>> # Find a specific process
            >>> process_info = client.find_process('550e8400-e29b-41d4-a716-446655440000')
            >>> if not process_info.empty:
            ...     print(f"Process: {process_info['exe'].iloc[0]}")
            ...     print(f"Started: {process_info['start_time'].iloc[0]}")
            ... else:
            ...     print("Process not found")

        Note:
            Returns an empty DataFrame if the process is not found.
        """
        sql = """
            SELECT *
            FROM processes
            WHERE process_id='{process_id}';
            """.format(
            process_id=process_id
        )
        return self.query(sql)

    def query_streams(self, begin, end, limit, process_id=None, tag_filter=None):
        """Query event streams with optional filtering.

        Retrieves information about event streams (collections of telemetry data)
        within a time range, with optional filtering by process or tags.

        Args:
            begin (datetime): Start time for the query (inclusive).
            end (datetime): End time for the query (exclusive).
            limit (int): Maximum number of streams to return.
            process_id (str, optional): Filter streams to a specific process.
            tag_filter (str, optional): Filter streams that contain a specific tag.
                Valid stream tags: 'log', 'metrics', 'cpu'.

        Returns:
            pandas.DataFrame: DataFrame containing stream information with columns:
                - stream_id: Unique stream identifier
                - process_id: Process that created the stream
                - stream_type: Type of stream (e.g., 'LOG', 'METRIC', 'SPAN')
                - tags: Array of tags associated with the stream
                - properties: Additional stream properties
                - time: Stream creation time

        Example:
            >>> from datetime import datetime, timedelta, timezone
            >>>
            >>> # Query all streams from the last hour
            >>> end = datetime.now(timezone.utc)
            >>> begin = end - timedelta(hours=1)
            >>> streams = client.query_streams(begin, end, limit=100)
            >>>
            >>> # Query streams for a specific process
            >>> streams = client.query_streams(
            ...     begin, end,
            ...     limit=50,
            ...     process_id='550e8400-e29b-41d4-a716-446655440000'
            ... )
            >>>
            >>> # Query streams with a specific tag
            >>> log_streams = client.query_streams(
            ...     begin, end,
            ...     limit=20,
            ...     tag_filter='log'
            ... )
        """
        conditions = []
        if process_id is not None:
            conditions.append("process_id='{process_id}'".format(process_id=process_id))
        if tag_filter is not None:
            conditions.append(
                "(array_position(tags, '{tag}') is not NULL)".format(tag=tag_filter)
            )
        where = ""
        if len(conditions) > 0:
            where = "WHERE " + " AND ".join(conditions)
        sql = """
            SELECT *
            FROM streams
            {where}
            LIMIT {limit};
            """.format(
            where=where, limit=limit
        )
        return self.query(sql, begin, end)

    def query_blocks(self, begin, end, limit, stream_id):
        """Query data blocks within a specific stream.

        Retrieves detailed information about data blocks (chunks of events) within
        a stream. Blocks are the fundamental storage unit for telemetry data.

        Args:
            begin (datetime): Start time for the query (inclusive).
            end (datetime): End time for the query (exclusive).
            limit (int): Maximum number of blocks to return.
            stream_id (str): The unique identifier of the stream to query.

        Returns:
            pandas.DataFrame: DataFrame containing block information with columns:
                - block_id: Unique block identifier
                - stream_id: Parent stream identifier
                - begin_time: Earliest event time in the block
                - end_time: Latest event time in the block
                - nb_events: Number of events in the block
                - payload_size: Size of the block in bytes
                - metadata: Additional block metadata

        Example:
            >>> # First, find a stream
            >>> streams = client.query_streams(begin, end, limit=1)
            >>> if not streams.empty:
            ...     stream_id = streams['stream_id'].iloc[0]
            ...
            ...     # Query blocks in that stream
            ...     blocks = client.query_blocks(begin, end, 100, stream_id)
            ...     print(f"Found {len(blocks)} blocks")
            ...     print(f"Total events: {blocks['nb_events'].sum()}")

        Note:
            Blocks are typically used for low-level data inspection and debugging.
            For normal queries, use higher-level methods like query() or query_stream().
        """
        sql = """
            SELECT *
            FROM blocks
            WHERE stream_id='{stream_id}'
            LIMIT {limit};
            """.format(
            limit=limit, stream_id=stream_id
        )
        return self.query(sql, begin, end)

    def query_spans(self, begin, end, limit, stream_id):
        """Query thread spans (execution traces) for a specific stream.

        Retrieves detailed span information showing the execution flow and timing
        of operations within a stream. Spans are hierarchical and represent
        function calls, operations, or logical units of work.

        Args:
            begin (datetime): Start time for the query (inclusive).
            end (datetime): End time for the query (exclusive).
            limit (int): Maximum number of spans to return.
            stream_id (str): The stream identifier to query spans from.

        Returns:
            pandas.DataFrame: DataFrame containing span information with columns:
                - span_id: Unique span identifier
                - parent_span_id: Parent span for hierarchical traces
                - name: Name of the operation or function
                - begin_time: When the span started
                - end_time: When the span completed
                - duration: Duration in nanoseconds
                - thread_id: Thread that executed the span
                - properties: Additional span attributes

        Example:
            >>> # Query spans to analyze performance
            >>> spans = client.query_spans(begin, end, 1000, stream_id)
            >>>
            >>> # Find slowest operations
            >>> slow_spans = spans.nlargest(10, 'duration')
            >>> for _, span in slow_spans.iterrows():
            ...     print(f"{span['name']}: {span['duration']/1000000:.2f}ms")
            >>>
            >>> # Analyze span hierarchy
            >>> root_spans = spans[spans['parent_span_id'].isna()]
            >>> print(f"Found {len(root_spans)} root spans")

        Note:
            Spans are essential for performance analysis and distributed tracing.
            Use with Perfetto trace generation for visualization.
        """
        sql = """
            SELECT *
            FROM view_instance('thread_spans', '{stream_id}')
            LIMIT {limit};
            """.format(
            limit=limit, stream_id=stream_id
        )
        return self.query(sql, begin, end)
