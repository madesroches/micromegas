# -*- coding: utf-8 -*-
# Generated by the protocol buffer compiler.  DO NOT EDIT!
# NO CHECKED-IN PROTOBUF GENCODE
# source: protos/perfetto/trace/ftrace/lwis.proto
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
    'protos/perfetto/trace/ftrace/lwis.proto'
)
# @@protoc_insertion_point(imports)

_sym_db = _symbol_database.Default()




DESCRIPTOR = _descriptor_pool.Default().AddSerializedFile(b'\n\'protos/perfetto/trace/ftrace/lwis.proto\x12\x0fperfetto.protos\"q\n\x1fLwisTracingMarkWriteFtraceEvent\x12\x11\n\tlwis_name\x18\x01 \x01(\t\x12\x0c\n\x04type\x18\x02 \x01(\r\x12\x0b\n\x03pid\x18\x03 \x01(\x05\x12\x11\n\tfunc_name\x18\x04 \x01(\t\x12\r\n\x05value\x18\x05 \x01(\x03')

_globals = globals()
_builder.BuildMessageAndEnumDescriptors(DESCRIPTOR, _globals)
_builder.BuildTopDescriptorsAndMessages(DESCRIPTOR, 'protos.perfetto.trace.ftrace.lwis_pb2', _globals)
if not _descriptor._USE_C_DESCRIPTORS:
  DESCRIPTOR._loaded_options = None
  _globals['_LWISTRACINGMARKWRITEFTRACEEVENT']._serialized_start=60
  _globals['_LWISTRACINGMARKWRITEFTRACEEVENT']._serialized_end=173
# @@protoc_insertion_point(module_scope)
