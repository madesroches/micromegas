
import micromegas
micromegas.load_perfetto_protos()
from protos.perfetto.trace import trace_pb2, trace_packet_pb2

trace = trace_pb2.Trace()
packets = trace.packet

process_id = 1
thread_uuid = 2
trusted_packet_sequence_id = 1

packet = trace_packet_pb2.TracePacket()
packet.track_descriptor.uuid = process_id
packet.track_descriptor.process.pid = 1234
packet.track_descriptor.process.process_name = "myprocess"
packets.append(packet)


packet = trace_packet_pb2.TracePacket()
packet.track_descriptor.uuid = thread_uuid
packet.track_descriptor.parent_uuid = 1
packet.track_descriptor.thread.pid = 1234
packet.track_descriptor.thread.tid = 1
packet.track_descriptor.thread.thread_name = "some thread"
packets.append(packet)

packet = trace_packet_pb2.TracePacket()
packet.timestamp = 100
packet.track_event.type = protos.perfetto.trace.track_event.track_event_pb2.TrackEvent.Type.TYPE_SLICE_BEGIN
packet.track_event.track_uuid = thread_uuid
packet.track_event.name = "span name"
packet.trusted_packet_sequence_id = trusted_packet_sequence_id
packets.append(packet)

packet = trace_packet_pb2.TracePacket()
packet.timestamp = 200
packet.track_event.type = protos.perfetto.trace.track_event.track_event_pb2.TrackEvent.Type.TYPE_SLICE_END
packet.track_event.track_uuid = thread_uuid
packet.trusted_packet_sequence_id = trusted_packet_sequence_id
packets.append(packet)

with open("trace.pb", "w") as f:
    f.write(trace.SerializeToString())
