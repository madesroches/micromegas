import certifi
import pyarrow
from pyarrow import flight
from typing import Any
import sys
from google.protobuf import any_pb2
from . import FlightSql_pb2
from . import time


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

def make_call_headers( begin, end ):
    call_headers = []
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
    return call_headers

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
    ingest_statement = FlightSql_pb2.CommandStatementIngest(table=table_name, temporary=False)
    desc = make_arrow_flight_descriptor(ingest_statement)
    return desc

class FlightSQLClient:
    def __init__(self, uri, headers=None):
        fh = open(certifi.where(), "r")
        cert = fh.read()
        fh.close()
        factory = MicromegasMiddlewareFactory(headers)
        self.__flight_client = flight.connect(
            location=uri, tls_root_certs=cert, middleware=[factory]
        )

    def query(self, sql, begin=None, end=None):
        call_headers = make_call_headers(begin, end)
        options = flight.FlightCallOptions(headers=call_headers)
        ticket = make_query_ticket(sql)
        reader = self.__flight_client.do_get(ticket, options=options)
        record_batches = []
        for chunk in reader:
            record_batches.append(chunk.data)
        table = pyarrow.Table.from_batches(record_batches, reader.schema)
        return table.to_pandas()

    def query_stream(self, sql, begin=None, end=None):
        ticket = make_query_ticket(sql)
        call_headers = make_call_headers(begin, end)
        options = flight.FlightCallOptions(headers=call_headers)
        reader = self.__flight_client.do_get(ticket, options=options)
        record_batches = []
        for chunk in reader:
            yield chunk.data

    def bulk_ingest(self, table_name, df):
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
        sql = """
            SELECT *
            FROM processes
            WHERE process_id='{process_id}';
            """.format(
            process_id=process_id
        )
        return self.query(sql)

    def query_streams(self, begin, end, limit, process_id=None, tag_filter=None):
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
        sql = """
            SELECT *
            FROM view_instance('thread_spans', '{stream_id}')
            LIMIT {limit};
            """.format(
            limit=limit, stream_id=stream_id
        )
        return self.query(sql, begin, end)
