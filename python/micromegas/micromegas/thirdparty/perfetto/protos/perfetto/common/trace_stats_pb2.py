# -*- coding: utf-8 -*-
# Generated by the protocol buffer compiler.  DO NOT EDIT!
# NO CHECKED-IN PROTOBUF GENCODE
# source: protos/perfetto/common/trace_stats.proto
# Protobuf Python Version: 5.27.1
"""Generated protocol buffer code."""
from google.protobuf import descriptor as _descriptor
from google.protobuf import descriptor_pool as _descriptor_pool
from google.protobuf import runtime_version as _runtime_version
from google.protobuf import symbol_database as _symbol_database
from google.protobuf.internal import builder as _builder
_runtime_version.ValidateProtobufRuntimeVersion(
    _runtime_version.Domain.PUBLIC,
    5,
    27,
    1,
    '',
    'protos/perfetto/common/trace_stats.proto'
)
# @@protoc_insertion_point(imports)

_sym_db = _symbol_database.Default()




DESCRIPTOR = _descriptor_pool.Default().AddSerializedFile(b'\n(protos/perfetto/common/trace_stats.proto\x12\x0fperfetto.protos\"\x91\x0c\n\nTraceStats\x12=\n\x0c\x62uffer_stats\x18\x01 \x03(\x0b\x32\'.perfetto.protos.TraceStats.BufferStats\x12#\n\x1b\x63hunk_payload_histogram_def\x18\x11 \x03(\x03\x12=\n\x0cwriter_stats\x18\x12 \x03(\x0b\x32\'.perfetto.protos.TraceStats.WriterStats\x12\x1b\n\x13producers_connected\x18\x02 \x01(\r\x12\x16\n\x0eproducers_seen\x18\x03 \x01(\x04\x12\x1f\n\x17\x64\x61ta_sources_registered\x18\x04 \x01(\r\x12\x19\n\x11\x64\x61ta_sources_seen\x18\x05 \x01(\x04\x12\x18\n\x10tracing_sessions\x18\x06 \x01(\r\x12\x15\n\rtotal_buffers\x18\x07 \x01(\r\x12\x18\n\x10\x63hunks_discarded\x18\x08 \x01(\x04\x12\x19\n\x11patches_discarded\x18\t \x01(\x04\x12\x17\n\x0finvalid_packets\x18\n \x01(\x04\x12=\n\x0c\x66ilter_stats\x18\x0b \x01(\x0b\x32\'.perfetto.protos.TraceStats.FilterStats\x12\x19\n\x11\x66lushes_requested\x18\x0c \x01(\x04\x12\x19\n\x11\x66lushes_succeeded\x18\r \x01(\x04\x12\x16\n\x0e\x66lushes_failed\x18\x0e \x01(\x04\x12J\n\x13\x66inal_flush_outcome\x18\x0f \x01(\x0e\x32-.perfetto.protos.TraceStats.FinalFlushOutcome\x1a\x8a\x04\n\x0b\x42ufferStats\x12\x13\n\x0b\x62uffer_size\x18\x0c \x01(\x04\x12\x15\n\rbytes_written\x18\x01 \x01(\x04\x12\x19\n\x11\x62ytes_overwritten\x18\r \x01(\x04\x12\x12\n\nbytes_read\x18\x0e \x01(\x04\x12\x1d\n\x15padding_bytes_written\x18\x0f \x01(\x04\x12\x1d\n\x15padding_bytes_cleared\x18\x10 \x01(\x04\x12\x16\n\x0e\x63hunks_written\x18\x02 \x01(\x04\x12\x18\n\x10\x63hunks_rewritten\x18\n \x01(\x04\x12\x1a\n\x12\x63hunks_overwritten\x18\x03 \x01(\x04\x12\x18\n\x10\x63hunks_discarded\x18\x12 \x01(\x04\x12\x13\n\x0b\x63hunks_read\x18\x11 \x01(\x04\x12%\n\x1d\x63hunks_committed_out_of_order\x18\x0b \x01(\x04\x12\x18\n\x10write_wrap_count\x18\x04 \x01(\x04\x12\x19\n\x11patches_succeeded\x18\x05 \x01(\x04\x12\x16\n\x0epatches_failed\x18\x06 \x01(\x04\x12\x1c\n\x14readaheads_succeeded\x18\x07 \x01(\x04\x12\x19\n\x11readaheads_failed\x18\x08 \x01(\x04\x12\x16\n\x0e\x61\x62i_violations\x18\t \x01(\x04\x12 \n\x18trace_writer_packet_loss\x18\x13 \x01(\x04\x1a\x87\x01\n\x0bWriterStats\x12\x13\n\x0bsequence_id\x18\x01 \x01(\x04\x12\x0e\n\x06\x62uffer\x18\x04 \x01(\r\x12*\n\x1e\x63hunk_payload_histogram_counts\x18\x02 \x03(\x04\x42\x02\x10\x01\x12\'\n\x1b\x63hunk_payload_histogram_sum\x18\x03 \x03(\x03\x42\x02\x10\x01\x1a\x9a\x01\n\x0b\x46ilterStats\x12\x15\n\rinput_packets\x18\x01 \x01(\x04\x12\x13\n\x0binput_bytes\x18\x02 \x01(\x04\x12\x14\n\x0coutput_bytes\x18\x03 \x01(\x04\x12\x0e\n\x06\x65rrors\x18\x04 \x01(\x04\x12\x15\n\rtime_taken_ns\x18\x05 \x01(\x04\x12\"\n\x1a\x62ytes_discarded_per_buffer\x18\x14 \x03(\x04\"c\n\x11\x46inalFlushOutcome\x12\x1b\n\x17\x46INAL_FLUSH_UNSPECIFIED\x10\x00\x12\x19\n\x15\x46INAL_FLUSH_SUCCEEDED\x10\x01\x12\x16\n\x12\x46INAL_FLUSH_FAILED\x10\x02')

_globals = globals()
_builder.BuildMessageAndEnumDescriptors(DESCRIPTOR, _globals)
_builder.BuildTopDescriptorsAndMessages(DESCRIPTOR, 'protos.perfetto.common.trace_stats_pb2', _globals)
if not _descriptor._USE_C_DESCRIPTORS:
  DESCRIPTOR._loaded_options = None
  _globals['_TRACESTATS_WRITERSTATS'].fields_by_name['chunk_payload_histogram_counts']._loaded_options = None
  _globals['_TRACESTATS_WRITERSTATS'].fields_by_name['chunk_payload_histogram_counts']._serialized_options = b'\020\001'
  _globals['_TRACESTATS_WRITERSTATS'].fields_by_name['chunk_payload_histogram_sum']._loaded_options = None
  _globals['_TRACESTATS_WRITERSTATS'].fields_by_name['chunk_payload_histogram_sum']._serialized_options = b'\020\001'
  _globals['_TRACESTATS']._serialized_start=62
  _globals['_TRACESTATS']._serialized_end=1615
  _globals['_TRACESTATS_BUFFERSTATS']._serialized_start=697
  _globals['_TRACESTATS_BUFFERSTATS']._serialized_end=1219
  _globals['_TRACESTATS_WRITERSTATS']._serialized_start=1222
  _globals['_TRACESTATS_WRITERSTATS']._serialized_end=1357
  _globals['_TRACESTATS_FILTERSTATS']._serialized_start=1360
  _globals['_TRACESTATS_FILTERSTATS']._serialized_end=1514
  _globals['_TRACESTATS_FINALFLUSHOUTCOME']._serialized_start=1516
  _globals['_TRACESTATS_FINALFLUSHOUTCOME']._serialized_end=1615
# @@protoc_insertion_point(module_scope)
