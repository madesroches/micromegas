# -*- coding: utf-8 -*-
# Generated by the protocol buffer compiler.  DO NOT EDIT!
# NO CHECKED-IN PROTOBUF GENCODE
# source: protos/perfetto/config/interceptors/console_config.proto
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
    'protos/perfetto/config/interceptors/console_config.proto'
)
# @@protoc_insertion_point(imports)

_sym_db = _symbol_database.Default()




DESCRIPTOR = _descriptor_pool.Default().AddSerializedFile(b'\n8protos/perfetto/config/interceptors/console_config.proto\x12\x0fperfetto.protos\"\xa5\x01\n\rConsoleConfig\x12\x35\n\x06output\x18\x01 \x01(\x0e\x32%.perfetto.protos.ConsoleConfig.Output\x12\x15\n\renable_colors\x18\x02 \x01(\x08\"F\n\x06Output\x12\x16\n\x12OUTPUT_UNSPECIFIED\x10\x00\x12\x11\n\rOUTPUT_STDOUT\x10\x01\x12\x11\n\rOUTPUT_STDERR\x10\x02')

_globals = globals()
_builder.BuildMessageAndEnumDescriptors(DESCRIPTOR, _globals)
_builder.BuildTopDescriptorsAndMessages(DESCRIPTOR, 'protos.perfetto.config.interceptors.console_config_pb2', _globals)
if not _descriptor._USE_C_DESCRIPTORS:
  DESCRIPTOR._loaded_options = None
  _globals['_CONSOLECONFIG']._serialized_start=78
  _globals['_CONSOLECONFIG']._serialized_end=243
  _globals['_CONSOLECONFIG_OUTPUT']._serialized_start=173
  _globals['_CONSOLECONFIG_OUTPUT']._serialized_end=243
# @@protoc_insertion_point(module_scope)
