# -*- coding: utf-8 -*-
# Generated by the protocol buffer compiler.  DO NOT EDIT!
# NO CHECKED-IN PROTOBUF GENCODE
# source: protos/perfetto/trace/ftrace/net.proto
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
    'protos/perfetto/trace/ftrace/net.proto'
)
# @@protoc_insertion_point(imports)

_sym_db = _symbol_database.Default()




DESCRIPTOR = _descriptor_pool.Default().AddSerializedFile(b'\n&protos/perfetto/trace/ftrace/net.proto\x12\x0fperfetto.protos\"H\n\x1aNetifReceiveSkbFtraceEvent\x12\x0b\n\x03len\x18\x01 \x01(\r\x12\x0c\n\x04name\x18\x02 \x01(\t\x12\x0f\n\x07skbaddr\x18\x03 \x01(\x04\"O\n\x15NetDevXmitFtraceEvent\x12\x0b\n\x03len\x18\x01 \x01(\r\x12\x0c\n\x04name\x18\x02 \x01(\t\x12\n\n\x02rc\x18\x03 \x01(\x05\x12\x0f\n\x07skbaddr\x18\x04 \x01(\x04\"\xfb\x02\n\x1eNapiGroReceiveEntryFtraceEvent\x12\x10\n\x08\x64\x61ta_len\x18\x01 \x01(\r\x12\x10\n\x08gso_size\x18\x02 \x01(\r\x12\x10\n\x08gso_type\x18\x03 \x01(\r\x12\x0c\n\x04hash\x18\x04 \x01(\r\x12\x11\n\tip_summed\x18\x05 \x01(\r\x12\x0f\n\x07l4_hash\x18\x06 \x01(\r\x12\x0b\n\x03len\x18\x07 \x01(\r\x12\x12\n\nmac_header\x18\x08 \x01(\x05\x12\x18\n\x10mac_header_valid\x18\t \x01(\r\x12\x0c\n\x04name\x18\n \x01(\t\x12\x0f\n\x07napi_id\x18\x0b \x01(\r\x12\x10\n\x08nr_frags\x18\x0c \x01(\r\x12\x10\n\x08protocol\x18\r \x01(\r\x12\x15\n\rqueue_mapping\x18\x0e \x01(\r\x12\x0f\n\x07skbaddr\x18\x0f \x01(\x04\x12\x10\n\x08truesize\x18\x10 \x01(\r\x12\x12\n\nvlan_proto\x18\x11 \x01(\r\x12\x13\n\x0bvlan_tagged\x18\x12 \x01(\r\x12\x10\n\x08vlan_tci\x18\x13 \x01(\r\",\n\x1dNapiGroReceiveExitFtraceEvent\x12\x0b\n\x03ret\x18\x01 \x01(\x05')

_globals = globals()
_builder.BuildMessageAndEnumDescriptors(DESCRIPTOR, _globals)
_builder.BuildTopDescriptorsAndMessages(DESCRIPTOR, 'protos.perfetto.trace.ftrace.net_pb2', _globals)
if not _descriptor._USE_C_DESCRIPTORS:
  DESCRIPTOR._loaded_options = None
  _globals['_NETIFRECEIVESKBFTRACEEVENT']._serialized_start=59
  _globals['_NETIFRECEIVESKBFTRACEEVENT']._serialized_end=131
  _globals['_NETDEVXMITFTRACEEVENT']._serialized_start=133
  _globals['_NETDEVXMITFTRACEEVENT']._serialized_end=212
  _globals['_NAPIGRORECEIVEENTRYFTRACEEVENT']._serialized_start=215
  _globals['_NAPIGRORECEIVEENTRYFTRACEEVENT']._serialized_end=594
  _globals['_NAPIGRORECEIVEEXITFTRACEEVENT']._serialized_start=596
  _globals['_NAPIGRORECEIVEEXITFTRACEEVENT']._serialized_end=640
# @@protoc_insertion_point(module_scope)
