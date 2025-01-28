import pyarrow
import grpc
from google.protobuf import any_pb2
from . import Flight_pb2_grpc
from . import FlightSql_pb2_grpc
from . import FlightSql_pb2
from . import Flight_pb2
from . import arrow_flatbuffers
from . import arrow_ipc_reader
from . import time


def fb_time_unit_to_string(fb_time_unit):
    time_unit_enum = arrow_flatbuffers.TimeUnit
    if fb_time_unit == time_unit_enum.SECOND:
        return "s"
    if fb_time_unit == time_unit_enum.MILLISECOND:
        return "ms"
    if fb_time_unit == time_unit_enum.MICROSECOND:
        return "us"
    if fb_time_unit == time_unit_enum.NANOSECOND:
        return "ns"
    raise RuntimeError("unsupported time unit {}".format(fb_time_unit))


def fb_field_type_to_arrow(fb_field):
    fb_type = fb_field.TypeType()
    fb_type_enum = arrow_flatbuffers.Type
    assert fb_type != fb_type_enum.NONE
    if fb_type == fb_type_enum.Null:
        return pyarrow.null()
    elif fb_type == fb_type_enum.Int:
        type_int = arrow_flatbuffers.Int()
        field_type_table = fb_field.Type()
        type_int.Init(field_type_table.Bytes, field_type_table.Pos)
        if type_int.IsSigned():
            if type_int.BitWidth() == 8:
                return pyarrow.int8()
            elif type_int.BitWidth() == 16:
                return pyarrow.int16()
            elif type_int.BitWidth() == 32:
                return pyarrow.int32()
            elif type_int.BitWidth() == 64:
                return pyarrow.int64()
            else:
                raise RuntimeError(
                    "unsupported int size {}".format(type_int.BitWidth())
                )
        else:
            if type_int.BitWidth() == 8:
                return pyarrow.uint8()
            elif type_int.BitWidth() == 16:
                return pyarrow.uint16()
            elif type_int.BitWidth() == 32:
                return pyarrow.uint32()
            elif type_int.BitWidth() == 64:
                return pyarrow.uint64()
            else:
                raise RuntimeError(
                    "unsupported uint size {}".format(type_int.BitWidth())
                )
    elif fb_type == fb_type_enum.FloatingPoint:
        return pyarrow.float64()
    elif fb_type == fb_type_enum.Binary:
        return pyarrow.binary()
    elif fb_type == fb_type_enum.Utf8:
        return pyarrow.utf8()
    elif fb_type == fb_type_enum.Bool:
        return pyarrow.bool()
    elif fb_type == fb_type_enum.Timestamp:
        ts_type = arrow_flatbuffers.Timestamp()
        field_type_table = fb_field.Type()
        ts_type.Init(field_type_table.Bytes, field_type_table.Pos)
        return pyarrow.timestamp(
            fb_time_unit_to_string(ts_type.Unit()), ts_type.Timezone()
        )
    elif fb_type == fb_type_enum.List:
        assert 1 == fb_field.ChildrenLength()
        child_field = fb_field_to_arrow(fb_field.Children(0))
        return pyarrow.list_(child_field)
    elif fb_type == fb_type_enum.Struct_:
        struct_fields = []
        for child_index in range(fb_field.ChildrenLength()):
            child = fb_field_to_arrow(fb_field.Children(child_index))
            struct_fields.append(child)
        return pyarrow.struct(struct_fields)
    raise RuntimeError("unknown flatbuffer type {}".format(fb_type))


def fb_field_to_arrow(fb_field):
    arrow_type = fb_field_type_to_arrow(fb_field)
    return pyarrow.field(fb_field.Name(), arrow_type)


def make_query_flight_descriptor(sql):
    command_query = FlightSql_pb2.CommandStatementQuery(query=sql)
    any_cmd = any_pb2.Any()
    any_cmd.Pack(command_query)
    desc = Flight_pb2.FlightDescriptor()
    desc.type = Flight_pb2.FlightDescriptor.DescriptorType.CMD
    desc.cmd = any_cmd.SerializeToString()
    return desc


def read_schema_from_flight_data(flight_data):
    msg = arrow_flatbuffers.Message.GetRootAs(flight_data.data_header, 0)
    assert msg.Version() == arrow_flatbuffers.MetadataVersion.V5
    header = msg.Header()
    assert msg.HeaderType() == arrow_flatbuffers.MessageHeader.Schema

    schema = arrow_flatbuffers.Schema()
    schema.Init(header.Bytes, header.Pos)
    nb_fields = schema.FieldsLength()
    arrow_fields = []
    for x in range(nb_fields):
        field = schema.Fields(x)
        arrow_f = fb_field_to_arrow(field)
        arrow_fields.append(arrow_f)
    arrow_schema = pyarrow.schema(arrow_fields)
    return arrow_schema


def read_record_batch_from_flight_data(arrow_schema, flight_data):
    msg = arrow_flatbuffers.Message.GetRootAs(flight_data.data_header, 0)
    assert msg.HeaderType() == arrow_flatbuffers.MessageHeader.RecordBatch
    header = msg.Header()
    fb_record_batch = arrow_flatbuffers.RecordBatch()
    fb_record_batch.Init(header.Bytes, header.Pos)
    nodes = []
    for node_index in range(fb_record_batch.NodesLength()):
        node = fb_record_batch.Nodes(node_index)
        nodes.append(node)

    buffers = []
    for buffer_index in range(fb_record_batch.BuffersLength()):
        buffer = fb_record_batch.Buffers(buffer_index)
        buffers.append(buffer)

    body = pyarrow.py_buffer(flight_data.data_body)
    arrow_buffers = []
    for b in buffers:
        s = body.slice(b.Offset(), b.Length())
        arrow_buffers.append(s)
    rb = arrow_ipc_reader.read_record_batch(arrow_schema, nodes, arrow_buffers)
    return rb


def channel_creds_from_token(token):
    call_credentials = grpc.access_token_call_credentials(token)
    channel_cred = grpc.composite_channel_credentials(
        grpc.ssl_channel_credentials(), call_credentials
    )
    return channel_cred


class FlightSQLAuthMetadataPlugin(grpc.AuthMetadataPlugin):
    def __init__(self, headers):
        # we transform the keys into lowercase to avoid illegal grpc metadata (like 'Authorization', for example)
        self.__headers = [(k.lower(), v) for (k, v) in headers.items()]

    def __call__(self, context, callback):
        callback(self.__headers, None)


def channel_creds_from_headers(headers):
    auth_plugin = FlightSQLAuthMetadataPlugin(headers)
    call_credentials = grpc.metadata_call_credentials(auth_plugin)
    channel_cred = grpc.composite_channel_credentials(
        grpc.ssl_channel_credentials(), call_credentials
    )
    return channel_cred


class FlightSQLClient:
    def __init__(self, host_port, channel_creds):
        self.__host_port = host_port
        self.__channel_creds = channel_creds

    def make_channel(self):
        if self.__channel_creds is None:
            return grpc.insecure_channel(self.__host_port)
        else:
            return grpc.secure_channel(self.__host_port, self.__channel_creds)

    def query(self, sql, begin=None, end=None):
        metadata = []
        if begin is not None:
            metadata.append(("query_range_begin", time.format_datetime(begin)))
        if end is not None:
            metadata.append(("query_range_end", time.format_datetime(end)))

        channel = self.make_channel()
        stub = Flight_pb2_grpc.FlightServiceStub(channel)
        desc = make_query_flight_descriptor(sql)
        info = stub.GetFlightInfo(desc)
        grpc_rdv = stub.DoGet(info.endpoint[0].ticket, metadata=metadata)
        flight_data_list = list(grpc_rdv)
        if len(flight_data_list) < 1:
            raise RuntimeError("too few flightdata messages {}", len(flight_data_list))
        schema_message = flight_data_list[0]
        data_messages = flight_data_list[1:]
        schema = read_schema_from_flight_data(schema_message)
        record_batches = []
        for msg in data_messages:
            record_batches.append(read_record_batch_from_flight_data(schema, msg))
        table = pyarrow.Table.from_batches(record_batches, schema)
        return table.to_pandas()

    def query_stream(self, sql, begin=None, end=None):
        metadata = []
        if begin is not None:
            metadata.append(("query_range_begin", time.format_datetime(begin)))
        if end is not None:
            metadata.append(("query_range_end", time.format_datetime(end)))

        channel = self.make_channel()
        stub = Flight_pb2_grpc.FlightServiceStub(channel)
        desc = make_query_flight_descriptor(sql)
        info = stub.GetFlightInfo(desc)
        grpc_rdv = stub.DoGet(info.endpoint[0].ticket, metadata=metadata)
        schema_message = grpc_rdv.next()
        schema = read_schema_from_flight_data(schema_message)
        for msg in grpc_rdv:
            yield read_record_batch_from_flight_data(schema, msg)

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
                limit=limit,stream_id=stream_id
        )
        return self.query(sql, begin, end)

    def query_spans(self, begin, end, limit, stream_id):
        sql = """
            SELECT *
            FROM view_instance('thread_spans', '{stream_id}')
            LIMIT {limit};
            """.format(
                limit=limit,stream_id=stream_id
        )
        return self.query(sql, begin, end)
    
