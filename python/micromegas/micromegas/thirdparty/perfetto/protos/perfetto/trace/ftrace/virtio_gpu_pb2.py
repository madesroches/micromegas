# -*- coding: utf-8 -*-
# Generated by the protocol buffer compiler.  DO NOT EDIT!
# NO CHECKED-IN PROTOBUF GENCODE
# source: protos/perfetto/trace/ftrace/virtio_gpu.proto
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
    'protos/perfetto/trace/ftrace/virtio_gpu.proto'
)
# @@protoc_insertion_point(imports)

_sym_db = _symbol_database.Default()




DESCRIPTOR = _descriptor_pool.Default().AddSerializedFile(b'\n-protos/perfetto/trace/ftrace/virtio_gpu.proto\x12\x0fperfetto.protos\"\xa5\x01\n\x1cVirtioGpuCmdQueueFtraceEvent\x12\x0e\n\x06\x63tx_id\x18\x01 \x01(\r\x12\x0b\n\x03\x64\x65v\x18\x02 \x01(\x05\x12\x10\n\x08\x66\x65nce_id\x18\x03 \x01(\x04\x12\r\n\x05\x66lags\x18\x04 \x01(\r\x12\x0c\n\x04name\x18\x05 \x01(\t\x12\x10\n\x08num_free\x18\x06 \x01(\r\x12\r\n\x05seqno\x18\x07 \x01(\r\x12\x0c\n\x04type\x18\x08 \x01(\r\x12\n\n\x02vq\x18\t \x01(\r\"\xa8\x01\n\x1fVirtioGpuCmdResponseFtraceEvent\x12\x0e\n\x06\x63tx_id\x18\x01 \x01(\r\x12\x0b\n\x03\x64\x65v\x18\x02 \x01(\x05\x12\x10\n\x08\x66\x65nce_id\x18\x03 \x01(\x04\x12\r\n\x05\x66lags\x18\x04 \x01(\r\x12\x0c\n\x04name\x18\x05 \x01(\t\x12\x10\n\x08num_free\x18\x06 \x01(\r\x12\r\n\x05seqno\x18\x07 \x01(\r\x12\x0c\n\x04type\x18\x08 \x01(\r\x12\n\n\x02vq\x18\t \x01(\r')

_globals = globals()
_builder.BuildMessageAndEnumDescriptors(DESCRIPTOR, _globals)
_builder.BuildTopDescriptorsAndMessages(DESCRIPTOR, 'protos.perfetto.trace.ftrace.virtio_gpu_pb2', _globals)
if not _descriptor._USE_C_DESCRIPTORS:
  DESCRIPTOR._loaded_options = None
  _globals['_VIRTIOGPUCMDQUEUEFTRACEEVENT']._serialized_start=67
  _globals['_VIRTIOGPUCMDQUEUEFTRACEEVENT']._serialized_end=232
  _globals['_VIRTIOGPUCMDRESPONSEFTRACEEVENT']._serialized_start=235
  _globals['_VIRTIOGPUCMDRESPONSEFTRACEEVENT']._serialized_end=403
# @@protoc_insertion_point(module_scope)
