import crc

# hack to allow perfetto proto imports
# you can then import the protos like this: from protos.perfetto.trace import trace_pb2
def load_perfetto_protos():
    import sys
    import pathlib
    perfetto_folder =  pathlib.Path(__file__).parent.absolute() / "thirdparty/perfetto"
    sys.path.append(str(perfetto_folder))

def crc64_str(s):
    calculator = crc.Calculator(crc.Crc64.CRC64)
    return calculator.checksum(str.encode(s))

class Writer:
    def __init__( self, client, process_id, exe ):
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

    def append_thread( self, begin, end, stream_id, thread_name, thread_id ):
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

        df_events = self.client.query_thread_events(begin, end, limit=1024*1024, stream_id = stream_id)
        df_events["ns"] = df_events["timestamp"].astype('int64')
        for index, event in df_events.iterrows():
            packet = trace_packet_pb2.TracePacket()
            packet.timestamp = event["ns"]
            if event["event_type"] == "begin":
                packet.track_event.type = track_event.track_event_pb2.TrackEvent.Type.TYPE_SLICE_BEGIN
            elif event["event_type"] == "end":
                packet.track_event.type = track_event.track_event_pb2.TrackEvent.Type.TYPE_SLICE_END
            else:
                raise Exception("unknown event type")
            packet.track_event.track_uuid = thread_uuid
            packet.track_event.name = event["name"]
            packet.track_event.categories.append(event["target"])
            packet.track_event.source_location.file_name = event["filename"]
            packet.track_event.source_location.line_number = event["line"]
            packet.trusted_packet_sequence_id = trusted_packet_sequence_id
            self.packets.append(packet)

    def write_file( self, filename ):
        with open(filename, "wb") as f:
            f.write(self.trace.SerializeToString())
        
        

        
