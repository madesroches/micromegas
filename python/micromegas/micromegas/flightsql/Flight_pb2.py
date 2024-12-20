# -*- coding: utf-8 -*-
# Generated by the protocol buffer compiler.  DO NOT EDIT!
# NO CHECKED-IN PROTOBUF GENCODE
# source: Flight.proto
# Protobuf Python Version: 5.29.1
"""Generated protocol buffer code."""
from google.protobuf import descriptor as _descriptor
from google.protobuf import descriptor_pool as _descriptor_pool
from google.protobuf import runtime_version as _runtime_version
from google.protobuf import symbol_database as _symbol_database
from google.protobuf.internal import builder as _builder
_runtime_version.ValidateProtobufRuntimeVersion(
    _runtime_version.Domain.PUBLIC,
    5,
    29,
    1,
    '',
    'Flight.proto'
)
# @@protoc_insertion_point(imports)

_sym_db = _symbol_database.Default()


from google.protobuf import timestamp_pb2 as google_dot_protobuf_dot_timestamp__pb2


DESCRIPTOR = _descriptor_pool.Default().AddSerializedFile(b'\n\x0c\x46light.proto\x12\x15\x61rrow.flight.protocol\x1a\x1fgoogle/protobuf/timestamp.proto\"=\n\x10HandshakeRequest\x12\x18\n\x10protocol_version\x18\x01 \x01(\x04\x12\x0f\n\x07payload\x18\x02 \x01(\x0c\">\n\x11HandshakeResponse\x12\x18\n\x10protocol_version\x18\x01 \x01(\x04\x12\x0f\n\x07payload\x18\x02 \x01(\x0c\"/\n\tBasicAuth\x12\x10\n\x08username\x18\x02 \x01(\t\x12\x10\n\x08password\x18\x03 \x01(\t\"\x07\n\x05\x45mpty\"/\n\nActionType\x12\x0c\n\x04type\x18\x01 \x01(\t\x12\x13\n\x0b\x64\x65scription\x18\x02 \x01(\t\"\x1e\n\x08\x43riteria\x12\x12\n\nexpression\x18\x01 \x01(\x0c\"$\n\x06\x41\x63tion\x12\x0c\n\x04type\x18\x01 \x01(\t\x12\x0c\n\x04\x62ody\x18\x02 \x01(\x0c\"\x16\n\x06Result\x12\x0c\n\x04\x62ody\x18\x01 \x01(\x0c\"\x1e\n\x0cSchemaResult\x12\x0e\n\x06schema\x18\x01 \x01(\x0c\"\xa5\x01\n\x10\x46lightDescriptor\x12\x44\n\x04type\x18\x01 \x01(\x0e\x32\x36.arrow.flight.protocol.FlightDescriptor.DescriptorType\x12\x0b\n\x03\x63md\x18\x02 \x01(\x0c\x12\x0c\n\x04path\x18\x03 \x03(\t\"0\n\x0e\x44\x65scriptorType\x12\x0b\n\x07UNKNOWN\x10\x00\x12\x08\n\x04PATH\x10\x01\x12\x07\n\x03\x43MD\x10\x02\"\xec\x01\n\nFlightInfo\x12\x0e\n\x06schema\x18\x01 \x01(\x0c\x12\x42\n\x11\x66light_descriptor\x18\x02 \x01(\x0b\x32\'.arrow.flight.protocol.FlightDescriptor\x12\x37\n\x08\x65ndpoint\x18\x03 \x03(\x0b\x32%.arrow.flight.protocol.FlightEndpoint\x12\x15\n\rtotal_records\x18\x04 \x01(\x03\x12\x13\n\x0btotal_bytes\x18\x05 \x01(\x03\x12\x0f\n\x07ordered\x18\x06 \x01(\x08\x12\x14\n\x0c\x61pp_metadata\x18\x07 \x01(\x0c\"\xd8\x01\n\x08PollInfo\x12/\n\x04info\x18\x01 \x01(\x0b\x32!.arrow.flight.protocol.FlightInfo\x12\x42\n\x11\x66light_descriptor\x18\x02 \x01(\x0b\x32\'.arrow.flight.protocol.FlightDescriptor\x12\x15\n\x08progress\x18\x03 \x01(\x01H\x00\x88\x01\x01\x12\x33\n\x0f\x65xpiration_time\x18\x04 \x01(\x0b\x32\x1a.google.protobuf.TimestampB\x0b\n\t_progress\"J\n\x17\x43\x61ncelFlightInfoRequest\x12/\n\x04info\x18\x01 \x01(\x0b\x32!.arrow.flight.protocol.FlightInfo\"M\n\x16\x43\x61ncelFlightInfoResult\x12\x33\n\x06status\x18\x01 \x01(\x0e\x32#.arrow.flight.protocol.CancelStatus\"\x18\n\x06Ticket\x12\x0e\n\x06ticket\x18\x01 \x01(\x0c\"\x17\n\x08Location\x12\x0b\n\x03uri\x18\x01 \x01(\t\"\xbd\x01\n\x0e\x46lightEndpoint\x12-\n\x06ticket\x18\x01 \x01(\x0b\x32\x1d.arrow.flight.protocol.Ticket\x12\x31\n\x08location\x18\x02 \x03(\x0b\x32\x1f.arrow.flight.protocol.Location\x12\x33\n\x0f\x65xpiration_time\x18\x03 \x01(\x0b\x32\x1a.google.protobuf.Timestamp\x12\x14\n\x0c\x61pp_metadata\x18\x04 \x01(\x0c\"U\n\x1aRenewFlightEndpointRequest\x12\x37\n\x08\x65ndpoint\x18\x01 \x01(\x0b\x32%.arrow.flight.protocol.FlightEndpoint\"\x8f\x01\n\nFlightData\x12\x42\n\x11\x66light_descriptor\x18\x01 \x01(\x0b\x32\'.arrow.flight.protocol.FlightDescriptor\x12\x13\n\x0b\x64\x61ta_header\x18\x02 \x01(\x0c\x12\x14\n\x0c\x61pp_metadata\x18\x03 \x01(\x0c\x12\x12\n\tdata_body\x18\xe8\x07 \x01(\x0c\"!\n\tPutResult\x12\x14\n\x0c\x61pp_metadata\x18\x01 \x01(\x0c\"\xfc\x01\n\x12SessionOptionValue\x12\x16\n\x0cstring_value\x18\x01 \x01(\tH\x00\x12\x14\n\nbool_value\x18\x02 \x01(\x08H\x00\x12\x15\n\x0bint64_value\x18\x03 \x01(\x10H\x00\x12\x16\n\x0c\x64ouble_value\x18\x04 \x01(\x01H\x00\x12V\n\x11string_list_value\x18\x05 \x01(\x0b\x32\x39.arrow.flight.protocol.SessionOptionValue.StringListValueH\x00\x1a!\n\x0fStringListValue\x12\x0e\n\x06values\x18\x01 \x03(\tB\x0e\n\x0coption_value\"\xda\x01\n\x18SetSessionOptionsRequest\x12\\\n\x0fsession_options\x18\x01 \x03(\x0b\x32\x43.arrow.flight.protocol.SetSessionOptionsRequest.SessionOptionsEntry\x1a`\n\x13SessionOptionsEntry\x12\x0b\n\x03key\x18\x01 \x01(\t\x12\x38\n\x05value\x18\x02 \x01(\x0b\x32).arrow.flight.protocol.SessionOptionValue:\x02\x38\x01\"\xec\x02\n\x17SetSessionOptionsResult\x12J\n\x06\x65rrors\x18\x01 \x03(\x0b\x32:.arrow.flight.protocol.SetSessionOptionsResult.ErrorsEntry\x1aQ\n\x05\x45rror\x12H\n\x05value\x18\x01 \x01(\x0e\x32\x39.arrow.flight.protocol.SetSessionOptionsResult.ErrorValue\x1a\x63\n\x0b\x45rrorsEntry\x12\x0b\n\x03key\x18\x01 \x01(\t\x12\x43\n\x05value\x18\x02 \x01(\x0b\x32\x34.arrow.flight.protocol.SetSessionOptionsResult.Error:\x02\x38\x01\"M\n\nErrorValue\x12\x0f\n\x0bUNSPECIFIED\x10\x00\x12\x10\n\x0cINVALID_NAME\x10\x01\x12\x11\n\rINVALID_VALUE\x10\x02\x12\t\n\x05\x45RROR\x10\x03\"\x1a\n\x18GetSessionOptionsRequest\"\xd8\x01\n\x17GetSessionOptionsResult\x12[\n\x0fsession_options\x18\x01 \x03(\x0b\x32\x42.arrow.flight.protocol.GetSessionOptionsResult.SessionOptionsEntry\x1a`\n\x13SessionOptionsEntry\x12\x0b\n\x03key\x18\x01 \x01(\t\x12\x38\n\x05value\x18\x02 \x01(\x0b\x32).arrow.flight.protocol.SessionOptionValue:\x02\x38\x01\"\x15\n\x13\x43loseSessionRequest\"\x9d\x01\n\x12\x43loseSessionResult\x12@\n\x06status\x18\x01 \x01(\x0e\x32\x30.arrow.flight.protocol.CloseSessionResult.Status\"E\n\x06Status\x12\x0f\n\x0bUNSPECIFIED\x10\x00\x12\n\n\x06\x43LOSED\x10\x01\x12\x0b\n\x07\x43LOSING\x10\x02\x12\x11\n\rNOT_CLOSEABLE\x10\x03*\x8b\x01\n\x0c\x43\x61ncelStatus\x12\x1d\n\x19\x43\x41NCEL_STATUS_UNSPECIFIED\x10\x00\x12\x1b\n\x17\x43\x41NCEL_STATUS_CANCELLED\x10\x01\x12\x1c\n\x18\x43\x41NCEL_STATUS_CANCELLING\x10\x02\x12!\n\x1d\x43\x41NCEL_STATUS_NOT_CANCELLABLE\x10\x03\x32\x85\x07\n\rFlightService\x12\x64\n\tHandshake\x12\'.arrow.flight.protocol.HandshakeRequest\x1a(.arrow.flight.protocol.HandshakeResponse\"\x00(\x01\x30\x01\x12U\n\x0bListFlights\x12\x1f.arrow.flight.protocol.Criteria\x1a!.arrow.flight.protocol.FlightInfo\"\x00\x30\x01\x12]\n\rGetFlightInfo\x12\'.arrow.flight.protocol.FlightDescriptor\x1a!.arrow.flight.protocol.FlightInfo\"\x00\x12\\\n\x0ePollFlightInfo\x12\'.arrow.flight.protocol.FlightDescriptor\x1a\x1f.arrow.flight.protocol.PollInfo\"\x00\x12[\n\tGetSchema\x12\'.arrow.flight.protocol.FlightDescriptor\x1a#.arrow.flight.protocol.SchemaResult\"\x00\x12M\n\x05\x44oGet\x12\x1d.arrow.flight.protocol.Ticket\x1a!.arrow.flight.protocol.FlightData\"\x00\x30\x01\x12R\n\x05\x44oPut\x12!.arrow.flight.protocol.FlightData\x1a .arrow.flight.protocol.PutResult\"\x00(\x01\x30\x01\x12X\n\nDoExchange\x12!.arrow.flight.protocol.FlightData\x1a!.arrow.flight.protocol.FlightData\"\x00(\x01\x30\x01\x12L\n\x08\x44oAction\x12\x1d.arrow.flight.protocol.Action\x1a\x1d.arrow.flight.protocol.Result\"\x00\x30\x01\x12R\n\x0bListActions\x12\x1c.arrow.flight.protocol.Empty\x1a!.arrow.flight.protocol.ActionType\"\x00\x30\x01\x42q\n\x1corg.apache.arrow.flight.implZ2github.com/apache/arrow-go/arrow/flight/gen/flight\xaa\x02\x1c\x41pache.Arrow.Flight.Protocolb\x06proto3')

_globals = globals()
_builder.BuildMessageAndEnumDescriptors(DESCRIPTOR, _globals)
_builder.BuildTopDescriptorsAndMessages(DESCRIPTOR, 'Flight_pb2', _globals)
if not _descriptor._USE_C_DESCRIPTORS:
  _globals['DESCRIPTOR']._loaded_options = None
  _globals['DESCRIPTOR']._serialized_options = b'\n\034org.apache.arrow.flight.implZ2github.com/apache/arrow-go/arrow/flight/gen/flight\252\002\034Apache.Arrow.Flight.Protocol'
  _globals['_SETSESSIONOPTIONSREQUEST_SESSIONOPTIONSENTRY']._loaded_options = None
  _globals['_SETSESSIONOPTIONSREQUEST_SESSIONOPTIONSENTRY']._serialized_options = b'8\001'
  _globals['_SETSESSIONOPTIONSRESULT_ERRORSENTRY']._loaded_options = None
  _globals['_SETSESSIONOPTIONSRESULT_ERRORSENTRY']._serialized_options = b'8\001'
  _globals['_GETSESSIONOPTIONSRESULT_SESSIONOPTIONSENTRY']._loaded_options = None
  _globals['_GETSESSIONOPTIONSRESULT_SESSIONOPTIONSENTRY']._serialized_options = b'8\001'
  _globals['_CANCELSTATUS']._serialized_start=2998
  _globals['_CANCELSTATUS']._serialized_end=3137
  _globals['_HANDSHAKEREQUEST']._serialized_start=72
  _globals['_HANDSHAKEREQUEST']._serialized_end=133
  _globals['_HANDSHAKERESPONSE']._serialized_start=135
  _globals['_HANDSHAKERESPONSE']._serialized_end=197
  _globals['_BASICAUTH']._serialized_start=199
  _globals['_BASICAUTH']._serialized_end=246
  _globals['_EMPTY']._serialized_start=248
  _globals['_EMPTY']._serialized_end=255
  _globals['_ACTIONTYPE']._serialized_start=257
  _globals['_ACTIONTYPE']._serialized_end=304
  _globals['_CRITERIA']._serialized_start=306
  _globals['_CRITERIA']._serialized_end=336
  _globals['_ACTION']._serialized_start=338
  _globals['_ACTION']._serialized_end=374
  _globals['_RESULT']._serialized_start=376
  _globals['_RESULT']._serialized_end=398
  _globals['_SCHEMARESULT']._serialized_start=400
  _globals['_SCHEMARESULT']._serialized_end=430
  _globals['_FLIGHTDESCRIPTOR']._serialized_start=433
  _globals['_FLIGHTDESCRIPTOR']._serialized_end=598
  _globals['_FLIGHTDESCRIPTOR_DESCRIPTORTYPE']._serialized_start=550
  _globals['_FLIGHTDESCRIPTOR_DESCRIPTORTYPE']._serialized_end=598
  _globals['_FLIGHTINFO']._serialized_start=601
  _globals['_FLIGHTINFO']._serialized_end=837
  _globals['_POLLINFO']._serialized_start=840
  _globals['_POLLINFO']._serialized_end=1056
  _globals['_CANCELFLIGHTINFOREQUEST']._serialized_start=1058
  _globals['_CANCELFLIGHTINFOREQUEST']._serialized_end=1132
  _globals['_CANCELFLIGHTINFORESULT']._serialized_start=1134
  _globals['_CANCELFLIGHTINFORESULT']._serialized_end=1211
  _globals['_TICKET']._serialized_start=1213
  _globals['_TICKET']._serialized_end=1237
  _globals['_LOCATION']._serialized_start=1239
  _globals['_LOCATION']._serialized_end=1262
  _globals['_FLIGHTENDPOINT']._serialized_start=1265
  _globals['_FLIGHTENDPOINT']._serialized_end=1454
  _globals['_RENEWFLIGHTENDPOINTREQUEST']._serialized_start=1456
  _globals['_RENEWFLIGHTENDPOINTREQUEST']._serialized_end=1541
  _globals['_FLIGHTDATA']._serialized_start=1544
  _globals['_FLIGHTDATA']._serialized_end=1687
  _globals['_PUTRESULT']._serialized_start=1689
  _globals['_PUTRESULT']._serialized_end=1722
  _globals['_SESSIONOPTIONVALUE']._serialized_start=1725
  _globals['_SESSIONOPTIONVALUE']._serialized_end=1977
  _globals['_SESSIONOPTIONVALUE_STRINGLISTVALUE']._serialized_start=1928
  _globals['_SESSIONOPTIONVALUE_STRINGLISTVALUE']._serialized_end=1961
  _globals['_SETSESSIONOPTIONSREQUEST']._serialized_start=1980
  _globals['_SETSESSIONOPTIONSREQUEST']._serialized_end=2198
  _globals['_SETSESSIONOPTIONSREQUEST_SESSIONOPTIONSENTRY']._serialized_start=2102
  _globals['_SETSESSIONOPTIONSREQUEST_SESSIONOPTIONSENTRY']._serialized_end=2198
  _globals['_SETSESSIONOPTIONSRESULT']._serialized_start=2201
  _globals['_SETSESSIONOPTIONSRESULT']._serialized_end=2565
  _globals['_SETSESSIONOPTIONSRESULT_ERROR']._serialized_start=2304
  _globals['_SETSESSIONOPTIONSRESULT_ERROR']._serialized_end=2385
  _globals['_SETSESSIONOPTIONSRESULT_ERRORSENTRY']._serialized_start=2387
  _globals['_SETSESSIONOPTIONSRESULT_ERRORSENTRY']._serialized_end=2486
  _globals['_SETSESSIONOPTIONSRESULT_ERRORVALUE']._serialized_start=2488
  _globals['_SETSESSIONOPTIONSRESULT_ERRORVALUE']._serialized_end=2565
  _globals['_GETSESSIONOPTIONSREQUEST']._serialized_start=2567
  _globals['_GETSESSIONOPTIONSREQUEST']._serialized_end=2593
  _globals['_GETSESSIONOPTIONSRESULT']._serialized_start=2596
  _globals['_GETSESSIONOPTIONSRESULT']._serialized_end=2812
  _globals['_GETSESSIONOPTIONSRESULT_SESSIONOPTIONSENTRY']._serialized_start=2102
  _globals['_GETSESSIONOPTIONSRESULT_SESSIONOPTIONSENTRY']._serialized_end=2198
  _globals['_CLOSESESSIONREQUEST']._serialized_start=2814
  _globals['_CLOSESESSIONREQUEST']._serialized_end=2835
  _globals['_CLOSESESSIONRESULT']._serialized_start=2838
  _globals['_CLOSESESSIONRESULT']._serialized_end=2995
  _globals['_CLOSESESSIONRESULT_STATUS']._serialized_start=2926
  _globals['_CLOSESESSIONRESULT_STATUS']._serialized_end=2995
  _globals['_FLIGHTSERVICE']._serialized_start=3140
  _globals['_FLIGHTSERVICE']._serialized_end=4041
# @@protoc_insertion_point(module_scope)