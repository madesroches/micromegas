# -*- coding: utf-8 -*-
# Generated by the protocol buffer compiler.  DO NOT EDIT!
# NO CHECKED-IN PROTOBUF GENCODE
# source: protos/perfetto/common/android_energy_consumer_descriptor.proto
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
    'protos/perfetto/common/android_energy_consumer_descriptor.proto'
)
# @@protoc_insertion_point(imports)

_sym_db = _symbol_database.Default()




DESCRIPTOR = _descriptor_pool.Default().AddSerializedFile(b'\n?protos/perfetto/common/android_energy_consumer_descriptor.proto\x12\x0fperfetto.protos\"`\n\x15\x41ndroidEnergyConsumer\x12\x1a\n\x12\x65nergy_consumer_id\x18\x01 \x01(\x05\x12\x0f\n\x07ordinal\x18\x02 \x01(\x05\x12\x0c\n\x04type\x18\x03 \x01(\t\x12\x0c\n\x04name\x18\x04 \x01(\t\"c\n\x1f\x41ndroidEnergyConsumerDescriptor\x12@\n\x10\x65nergy_consumers\x18\x01 \x03(\x0b\x32&.perfetto.protos.AndroidEnergyConsumer')

_globals = globals()
_builder.BuildMessageAndEnumDescriptors(DESCRIPTOR, _globals)
_builder.BuildTopDescriptorsAndMessages(DESCRIPTOR, 'protos.perfetto.common.android_energy_consumer_descriptor_pb2', _globals)
if not _descriptor._USE_C_DESCRIPTORS:
  DESCRIPTOR._loaded_options = None
  _globals['_ANDROIDENERGYCONSUMER']._serialized_start=84
  _globals['_ANDROIDENERGYCONSUMER']._serialized_end=180
  _globals['_ANDROIDENERGYCONSUMERDESCRIPTOR']._serialized_start=182
  _globals['_ANDROIDENERGYCONSUMERDESCRIPTOR']._serialized_end=281
# @@protoc_insertion_point(module_scope)
