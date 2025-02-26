import crc
import pyarrow
from tqdm import tqdm


# hack to allow perfetto proto imports
# you can then import the protos like this: from protos.perfetto.trace import trace_pb2
def load_perfetto_protos():
    import sys
    import pathlib

    perfetto_folder = pathlib.Path(__file__).parent.absolute() / "thirdparty/perfetto"
    sys.path.append(str(perfetto_folder))


def crc64_str(s):
    calculator = crc.Calculator(crc.Crc64.CRC64)
    return calculator.checksum(str.encode(s))


class Writer:
    """
    Fetches thread events from the analytics server and formats them in the perfetto format.
    Traces can be viewed using https://ui.perfetto.dev/
    """

    def __init__(self, client, process_id, begin, end, exe):
        load_perfetto_protos()
        from protos.perfetto.trace import trace_pb2, trace_packet_pb2

        self.names = {}
        self.categories = {}
        self.source_locations = {}
        self.first = True
        self.client = client
        self.trace = trace_pb2.Trace()
        self.packets = self.trace.packet
        self.process_uuid = crc64_str(process_id)
        self.begin = begin
        self.end = end

        packet = trace_packet_pb2.TracePacket()
        packet.track_descriptor.uuid = self.process_uuid
        packet.track_descriptor.process.pid = 1
        packet.track_descriptor.process.process_name = exe
        self.packets.append(packet)

    def get_name_iid(self, name):
        iid = self.names.get(name)
        is_new = False
        if iid is None:
            is_new = True
            iid = len(self.names) + 1
            self.names[name] = iid
        return iid, is_new

    def get_category_iid(self, cat):
        iid = self.categories.get(cat)
        is_new = False
        if iid is None:
            is_new = True
            iid = len(self.categories) + 1
            self.categories[cat] = iid
        return iid, is_new

    def get_location_iid(self, loc):
        iid = self.source_locations.get(loc)
        is_new = False
        if iid is None:
            is_new = True
            iid = len(self.source_locations) + 1
            self.source_locations[loc] = iid
        return iid, is_new

    def append_thread(self, stream_id, thread_name, thread_id):
        from protos.perfetto.trace import trace_pb2, trace_packet_pb2, track_event

        packet = trace_packet_pb2.TracePacket()
        thread_uuid = crc64_str(stream_id)
        packet.track_descriptor.uuid = thread_uuid
        packet.track_descriptor.parent_uuid = self.process_uuid
        packet.track_descriptor.thread.pid = 1
        packet.track_descriptor.thread.tid = thread_id
        packet.track_descriptor.thread.thread_name = thread_name
        self.packets.append(packet)
        trusted_packet_sequence_id = 1

        sql = """
          SELECT *
          FROM view_instance('thread_spans', '{stream_id}');
        """.format(
            stream_id=stream_id
        )

        for rb_spans in self.client.query_stream(sql, self.begin, self.end):
            df_spans = pyarrow.Table.from_batches([rb_spans]).to_pandas()
            begin_ns = df_spans["begin"].astype("int64")
            end_ns = df_spans["end"].astype("int64")
            for index, span in df_spans.iterrows():
                packet = trace_packet_pb2.TracePacket()
                packet.timestamp = begin_ns[index]
                packet.track_event.type = (
                    track_event.track_event_pb2.TrackEvent.Type.TYPE_SLICE_BEGIN
                )
                packet.track_event.track_uuid = thread_uuid
                span_name = span["name"]
                name_iid, new_name = self.get_name_iid(span_name)
                packet.track_event.name_iid = name_iid
                category_iid, new_category = self.get_category_iid(span["target"])
                packet.track_event.category_iids.append(category_iid)

                source_location = (span["filename"], span["line"])
                source_location_iid, new_source_location = self.get_location_iid(
                    source_location
                )
                packet.track_event.source_location_iid = source_location_iid
                if self.first:
                    # this is necessary for interning to work
                    self.first = False
                    packet.first_packet_on_sequence = True
                    packet.sequence_flags = 3
                else:
                    packet.sequence_flags = 2

                if new_name:
                    event_name = packet.interned_data.event_names.add()
                    event_name.iid = name_iid
                    event_name.name = span_name
                if new_category:
                    cat_name = packet.interned_data.event_categories.add()
                    cat_name.iid = category_iid
                    cat_name.name = span["target"]
                if new_source_location:
                    loc = packet.interned_data.source_locations.add()
                    loc.iid = source_location_iid
                    loc.file_name = source_location[0]
                    loc.line_number = source_location[1]

                packet.trusted_packet_sequence_id = trusted_packet_sequence_id
                self.packets.append(packet)

                packet = trace_packet_pb2.TracePacket()
                packet.timestamp = end_ns[index]
                packet.track_event.type = (
                    track_event.track_event_pb2.TrackEvent.Type.TYPE_SLICE_END
                )
                packet.track_event.track_uuid = thread_uuid
                packet.track_event.name_iid = name_iid
                packet.track_event.category_iids.append(category_iid)
                packet.track_event.source_location_iid = source_location_iid
                packet.sequence_flags = 2
                packet.trusted_packet_sequence_id = trusted_packet_sequence_id

                self.packets.append(packet)

    def write_file(self, filename):
        with open(filename, "wb") as f:
            f.write(self.trace.SerializeToString())


def get_process_cpu_streams(client, process_id, begin, end):
    sql = """
      SELECT stream_id,
             property_get("streams.properties", 'thread-name') as thread_name,
             property_get("streams.properties", 'thread-id') as thread_id
      FROM blocks
      WHERE process_id = '{process_id}'
      AND array_has("streams.tags", 'cpu')
      GROUP BY stream_id, thread_name, thread_id
    """.format(
        process_id=process_id
    )
    df_streams = client.query(sql)
    return df_streams


def get_exe(client, process_id, begin, end):
    sql = """
      SELECT "processes.exe" as exe
      FROM blocks
      WHERE process_id='{process_id}'
      LIMIT 1;""".format(
        process_id=process_id
    )
    return client.query(sql, begin, end).iloc[0]["exe"]


def write_process_trace(client, process_id, begin, end, trace_filepath):
    exe = get_exe(client, process_id, begin, end)
    print(exe)
    streams = get_process_cpu_streams(client, process_id, begin, end)
    writer = Writer(client, process_id, begin, end, exe)
    progress_bar = tqdm(list(streams.iterrows()), unit="threads")
    for index, stream in progress_bar:
        progress_bar.set_description(stream["thread_name"])
        stream_id = int(stream["thread_id"])
        writer.append_thread(stream["stream_id"], stream["thread_name"], stream_id)
    writer.write_file(trace_filepath)
