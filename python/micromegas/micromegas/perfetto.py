import crc
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

    def __init__(self, client, process_id, exe):
        load_perfetto_protos()
        from protos.perfetto.trace import trace_pb2, trace_packet_pb2

        self.client = client
        self.trace = trace_pb2.Trace()
        self.packets = self.trace.packet
        self.process_uuid = crc64_str(process_id)

        packet = trace_packet_pb2.TracePacket()
        packet.track_descriptor.uuid = self.process_uuid
        packet.track_descriptor.process.pid = 1
        packet.track_descriptor.process.process_name = exe
        self.packets.append(packet)

    def append_thread(self, stream_id, thread_name, thread_id):
        from protos.perfetto.trace import trace_pb2, trace_packet_pb2, track_event

        df_blocks = self.client.query_blocks(
            begin=None, end=None, limit=100_000, stream_id=stream_id
        )
        if df_blocks.empty:
            return
        begin = df_blocks["begin_time"].min()
        end = df_blocks["end_time"].max()

        packet = trace_packet_pb2.TracePacket()
        thread_uuid = crc64_str(stream_id)
        packet.track_descriptor.uuid = thread_uuid
        packet.track_descriptor.parent_uuid = self.process_uuid
        packet.track_descriptor.thread.pid = 1
        packet.track_descriptor.thread.tid = thread_id
        packet.track_descriptor.thread.thread_name = thread_name
        self.packets.append(packet)
        trusted_packet_sequence_id = 1

        max_rows = 1024 * 1024
        df_spans = self.client.query_spans(
            begin, end, limit=max_rows, stream_id=stream_id
        )
        nb_rows = df_spans.shape[0]
        if nb_rows == max_rows:
            print("Warning: partial data returned, needs multiple requests")

        begin_ns = df_spans["begin"].astype("int64")
        end_ns = df_spans["end"].astype("int64")
        for index, span in df_spans.iterrows():
            packet = trace_packet_pb2.TracePacket()
            packet.timestamp = begin_ns[index]
            packet.track_event.type = (
                track_event.track_event_pb2.TrackEvent.Type.TYPE_SLICE_BEGIN
            )
            packet.track_event.track_uuid = thread_uuid
            packet.track_event.name = span["name"]
            packet.track_event.categories.append(span["target"])
            packet.track_event.source_location.file_name = span["filename"]
            packet.track_event.source_location.line_number = span["line"]
            packet.trusted_packet_sequence_id = trusted_packet_sequence_id
            self.packets.append(packet)

            packet = trace_packet_pb2.TracePacket()
            packet.timestamp = end_ns[index]
            packet.track_event.type = (
                track_event.track_event_pb2.TrackEvent.Type.TYPE_SLICE_END
            )
            packet.track_event.track_uuid = thread_uuid
            packet.track_event.name = span["name"]
            packet.track_event.categories.append(span["target"])
            packet.track_event.source_location.file_name = span["filename"]
            packet.track_event.source_location.line_number = span["line"]
            packet.trusted_packet_sequence_id = trusted_packet_sequence_id
            self.packets.append(packet)

    def write_file(self, filename):
        with open(filename, "wb") as f:
            f.write(self.trace.SerializeToString())


def get_process_cpu_streams(client, process_id):
    def prop_to_dict(props):
        prop_dict = {}
        for p in props:
            prop_dict[p["key"]] = p["value"]
        return prop_dict

    def get_thread_name(prop_dict):
        return prop_dict["thread-name"]

    def get_thread_id(prop_dict):
        return int(prop_dict["thread-id"])

    df_streams = client.query_streams(
        begin=None, end=None, limit=1024, tag_filter="cpu", process_id=process_id
    )
    df_streams["properties"] = df_streams["properties"].apply(prop_to_dict)
    df_streams["thread_name"] = df_streams["properties"].apply(get_thread_name)
    df_streams["thread_id"] = df_streams["properties"].apply(get_thread_id)
    return df_streams


def write_process_trace(client, process_id, trace_filepath):
    process_df = client.find_process(process_id)
    assert process_df.shape[0] == 1
    process = process_df.iloc[0]
    streams = get_process_cpu_streams(client, process_id)
    writer = Writer(client, process_id, process["exe"])
    for index, stream in tqdm(list(streams.iterrows())):
        stream_id = stream["thread_id"]
        writer.append_thread(stream["stream_id"], stream["thread_name"], stream_id)
    writer.write_file(trace_filepath)
