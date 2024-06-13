# -*- coding: utf-8 -*-
# Generated by the protocol buffer compiler.  DO NOT EDIT!
# NO CHECKED-IN PROTOBUF GENCODE
# source: protos/perfetto/config/trace_config.proto
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
    'protos/perfetto/config/trace_config.proto'
)
# @@protoc_insertion_point(imports)

_sym_db = _symbol_database.Default()


from protos.perfetto.common import builtin_clock_pb2 as protos_dot_perfetto_dot_common_dot_builtin__clock__pb2
from protos.perfetto.config import data_source_config_pb2 as protos_dot_perfetto_dot_config_dot_data__source__config__pb2


DESCRIPTOR = _descriptor_pool.Default().AddSerializedFile(b'\n)protos/perfetto/config/trace_config.proto\x12\x0fperfetto.protos\x1a*protos/perfetto/common/builtin_clock.proto\x1a/protos/perfetto/config/data_source_config.proto\"\xe9#\n\x0bTraceConfig\x12:\n\x07\x62uffers\x18\x01 \x03(\x0b\x32).perfetto.protos.TraceConfig.BufferConfig\x12=\n\x0c\x64\x61ta_sources\x18\x02 \x03(\x0b\x32\'.perfetto.protos.TraceConfig.DataSource\x12L\n\x14\x62uiltin_data_sources\x18\x14 \x01(\x0b\x32..perfetto.protos.TraceConfig.BuiltinDataSource\x12\x13\n\x0b\x64uration_ms\x18\x03 \x01(\r\x12)\n!prefer_suspend_clock_for_duration\x18$ \x01(\x08\x12\x1f\n\x17\x65nable_extra_guardrails\x18\x04 \x01(\x08\x12I\n\rlockdown_mode\x18\x05 \x01(\x0e\x32\x32.perfetto.protos.TraceConfig.LockdownModeOperation\x12>\n\tproducers\x18\x06 \x03(\x0b\x32+.perfetto.protos.TraceConfig.ProducerConfig\x12\x44\n\x0fstatsd_metadata\x18\x07 \x01(\x0b\x32+.perfetto.protos.TraceConfig.StatsdMetadata\x12\x17\n\x0fwrite_into_file\x18\x08 \x01(\x08\x12\x13\n\x0boutput_path\x18\x1d \x01(\t\x12\x1c\n\x14\x66ile_write_period_ms\x18\t \x01(\r\x12\x1b\n\x13max_file_size_bytes\x18\n \x01(\x04\x12L\n\x13guardrail_overrides\x18\x0b \x01(\x0b\x32/.perfetto.protos.TraceConfig.GuardrailOverrides\x12\x16\n\x0e\x64\x65\x66\x65rred_start\x18\x0c \x01(\x08\x12\x17\n\x0f\x66lush_period_ms\x18\r \x01(\r\x12\x18\n\x10\x66lush_timeout_ms\x18\x0e \x01(\r\x12#\n\x1b\x64\x61ta_source_stop_timeout_ms\x18\x17 \x01(\r\x12\x16\n\x0enotify_traceur\x18\x10 \x01(\x08\x12\x17\n\x0f\x62ugreport_score\x18\x1e \x01(\x05\x12\x1a\n\x12\x62ugreport_filename\x18& \x01(\t\x12\x42\n\x0etrigger_config\x18\x11 \x01(\x0b\x32*.perfetto.protos.TraceConfig.TriggerConfig\x12\x19\n\x11\x61\x63tivate_triggers\x18\x12 \x03(\t\x12U\n\x18incremental_state_config\x18\x15 \x01(\x0b\x32\x33.perfetto.protos.TraceConfig.IncrementalStateConfig\x12 \n\x18\x61llow_user_build_tracing\x18\x13 \x01(\x08\x12\x1b\n\x13unique_session_name\x18\x16 \x01(\t\x12\x46\n\x10\x63ompression_type\x18\x18 \x01(\x0e\x32,.perfetto.protos.TraceConfig.CompressionType\x12Q\n\x16incident_report_config\x18\x19 \x01(\x0b\x32\x31.perfetto.protos.TraceConfig.IncidentReportConfig\x12\x42\n\x0estatsd_logging\x18\x1f \x01(\x0e\x32*.perfetto.protos.TraceConfig.StatsdLogging\x12\x1a\n\x0etrace_uuid_msb\x18\x1b \x01(\x03\x42\x02\x18\x01\x12\x1a\n\x0etrace_uuid_lsb\x18\x1c \x01(\x03\x42\x02\x18\x01\x12>\n\x0ctrace_filter\x18! \x01(\x0b\x32(.perfetto.protos.TraceConfig.TraceFilter\x12O\n\x15\x61ndroid_report_config\x18\" \x01(\x0b\x32\x30.perfetto.protos.TraceConfig.AndroidReportConfig\x12N\n\x15\x63md_trace_start_delay\x18# \x01(\x0b\x32/.perfetto.protos.TraceConfig.CmdTraceStartDelay\x12I\n\x12session_semaphores\x18\' \x03(\x0b\x32-.perfetto.protos.TraceConfig.SessionSemaphore\x1a\xea\x01\n\x0c\x42ufferConfig\x12\x0f\n\x07size_kb\x18\x01 \x01(\r\x12I\n\x0b\x66ill_policy\x18\x04 \x01(\x0e\x32\x34.perfetto.protos.TraceConfig.BufferConfig.FillPolicy\x12\x19\n\x11transfer_on_clone\x18\x05 \x01(\x08\x12\x1a\n\x12\x63lear_before_clone\x18\x06 \x01(\x08\";\n\nFillPolicy\x12\x0f\n\x0bUNSPECIFIED\x10\x00\x12\x0f\n\x0bRING_BUFFER\x10\x01\x12\x0b\n\x07\x44ISCARD\x10\x02J\x04\x08\x02\x10\x03J\x04\x08\x03\x10\x04\x1a\x81\x01\n\nDataSource\x12\x31\n\x06\x63onfig\x18\x01 \x01(\x0b\x32!.perfetto.protos.DataSourceConfig\x12\x1c\n\x14producer_name_filter\x18\x02 \x03(\t\x12\"\n\x1aproducer_name_regex_filter\x18\x03 \x03(\t\x1a\xbf\x02\n\x11\x42uiltinDataSource\x12\"\n\x1a\x64isable_clock_snapshotting\x18\x01 \x01(\x08\x12\x1c\n\x14\x64isable_trace_config\x18\x02 \x01(\x08\x12\x1b\n\x13\x64isable_system_info\x18\x03 \x01(\x08\x12\x1e\n\x16\x64isable_service_events\x18\x04 \x01(\x08\x12:\n\x13primary_trace_clock\x18\x05 \x01(\x0e\x32\x1d.perfetto.protos.BuiltinClock\x12\x1c\n\x14snapshot_interval_ms\x18\x06 \x01(\r\x12)\n!prefer_suspend_clock_for_snapshot\x18\x07 \x01(\x08\x12&\n\x1e\x64isable_chunk_usage_histograms\x18\x08 \x01(\x08\x1aR\n\x0eProducerConfig\x12\x15\n\rproducer_name\x18\x01 \x01(\t\x12\x13\n\x0bshm_size_kb\x18\x02 \x01(\r\x12\x14\n\x0cpage_size_kb\x18\x03 \x01(\r\x1a\x8e\x01\n\x0eStatsdMetadata\x12\x1b\n\x13triggering_alert_id\x18\x01 \x01(\x03\x12\x1d\n\x15triggering_config_uid\x18\x02 \x01(\x05\x12\x1c\n\x14triggering_config_id\x18\x03 \x01(\x03\x12\"\n\x1atriggering_subscription_id\x18\x04 \x01(\x03\x1a^\n\x12GuardrailOverrides\x12$\n\x18max_upload_per_day_bytes\x18\x01 \x01(\x04\x42\x02\x18\x01\x12\"\n\x1amax_tracing_buffer_size_kb\x18\x02 \x01(\r\x1a\xca\x03\n\rTriggerConfig\x12L\n\x0ctrigger_mode\x18\x01 \x01(\x0e\x32\x36.perfetto.protos.TraceConfig.TriggerConfig.TriggerMode\x12\'\n\x1fuse_clone_snapshot_if_available\x18\x05 \x01(\x08\x12\x44\n\x08triggers\x18\x02 \x03(\x0b\x32\x32.perfetto.protos.TraceConfig.TriggerConfig.Trigger\x12\x1a\n\x12trigger_timeout_ms\x18\x03 \x01(\r\x1a{\n\x07Trigger\x12\x0c\n\x04name\x18\x01 \x01(\t\x12\x1b\n\x13producer_name_regex\x18\x02 \x01(\t\x12\x15\n\rstop_delay_ms\x18\x03 \x01(\r\x12\x14\n\x0cmax_per_24_h\x18\x04 \x01(\r\x12\x18\n\x10skip_probability\x18\x05 \x01(\x01\"]\n\x0bTriggerMode\x12\x0f\n\x0bUNSPECIFIED\x10\x00\x12\x11\n\rSTART_TRACING\x10\x01\x12\x10\n\x0cSTOP_TRACING\x10\x02\x12\x12\n\x0e\x43LONE_SNAPSHOT\x10\x04\"\x04\x08\x03\x10\x03J\x04\x08\x04\x10\x05\x1a\x31\n\x16IncrementalStateConfig\x12\x17\n\x0f\x63lear_period_ms\x18\x01 \x01(\r\x1a\x97\x01\n\x14IncidentReportConfig\x12\x1b\n\x13\x64\x65stination_package\x18\x01 \x01(\t\x12\x19\n\x11\x64\x65stination_class\x18\x02 \x01(\t\x12\x15\n\rprivacy_level\x18\x03 \x01(\x05\x12\x16\n\x0eskip_incidentd\x18\x05 \x01(\x08\x12\x18\n\x0cskip_dropbox\x18\x04 \x01(\x08\x42\x02\x18\x01\x1a\xd5\x04\n\x0bTraceFilter\x12\x10\n\x08\x62ytecode\x18\x01 \x01(\x0c\x12\x13\n\x0b\x62ytecode_v2\x18\x02 \x01(\x0c\x12W\n\x13string_filter_chain\x18\x03 \x01(\x0b\x32:.perfetto.protos.TraceConfig.TraceFilter.StringFilterChain\x1a\x9a\x01\n\x10StringFilterRule\x12K\n\x06policy\x18\x01 \x01(\x0e\x32;.perfetto.protos.TraceConfig.TraceFilter.StringFilterPolicy\x12\x15\n\rregex_pattern\x18\x02 \x01(\t\x12\"\n\x1a\x61trace_payload_starts_with\x18\x03 \x01(\t\x1a]\n\x11StringFilterChain\x12H\n\x05rules\x18\x01 \x03(\x0b\x32\x39.perfetto.protos.TraceConfig.TraceFilter.StringFilterRule\"\xc9\x01\n\x12StringFilterPolicy\x12\x13\n\x0fSFP_UNSPECIFIED\x10\x00\x12\x1b\n\x17SFP_MATCH_REDACT_GROUPS\x10\x01\x12\"\n\x1eSFP_ATRACE_MATCH_REDACT_GROUPS\x10\x02\x12\x13\n\x0fSFP_MATCH_BREAK\x10\x03\x12\x1a\n\x16SFP_ATRACE_MATCH_BREAK\x10\x04\x12,\n(SFP_ATRACE_REPEATED_SEARCH_REDACT_GROUPS\x10\x05\x1a\x97\x01\n\x13\x41ndroidReportConfig\x12 \n\x18reporter_service_package\x18\x01 \x01(\t\x12\x1e\n\x16reporter_service_class\x18\x02 \x01(\t\x12\x13\n\x0bskip_report\x18\x03 \x01(\x08\x12)\n!use_pipe_in_framework_for_testing\x18\x04 \x01(\x08\x1a@\n\x12\x43mdTraceStartDelay\x12\x14\n\x0cmin_delay_ms\x18\x01 \x01(\r\x12\x14\n\x0cmax_delay_ms\x18\x02 \x01(\r\x1a\x41\n\x10SessionSemaphore\x12\x0c\n\x04name\x18\x01 \x01(\t\x12\x1f\n\x17max_other_session_count\x18\x02 \x01(\x04\"U\n\x15LockdownModeOperation\x12\x16\n\x12LOCKDOWN_UNCHANGED\x10\x00\x12\x12\n\x0eLOCKDOWN_CLEAR\x10\x01\x12\x10\n\x0cLOCKDOWN_SET\x10\x02\"Q\n\x0f\x43ompressionType\x12 \n\x1c\x43OMPRESSION_TYPE_UNSPECIFIED\x10\x00\x12\x1c\n\x18\x43OMPRESSION_TYPE_DEFLATE\x10\x01\"h\n\rStatsdLogging\x12\x1e\n\x1aSTATSD_LOGGING_UNSPECIFIED\x10\x00\x12\x1a\n\x16STATSD_LOGGING_ENABLED\x10\x01\x12\x1b\n\x17STATSD_LOGGING_DISABLED\x10\x02J\x04\x08\x0f\x10\x10J\x04\x08%\x10&J\x04\x08\x1a\x10\x1bJ\x04\x08 \x10!')

_globals = globals()
_builder.BuildMessageAndEnumDescriptors(DESCRIPTOR, _globals)
_builder.BuildTopDescriptorsAndMessages(DESCRIPTOR, 'protos.perfetto.config.trace_config_pb2', _globals)
if not _descriptor._USE_C_DESCRIPTORS:
  DESCRIPTOR._loaded_options = None
  _globals['_TRACECONFIG_GUARDRAILOVERRIDES'].fields_by_name['max_upload_per_day_bytes']._loaded_options = None
  _globals['_TRACECONFIG_GUARDRAILOVERRIDES'].fields_by_name['max_upload_per_day_bytes']._serialized_options = b'\030\001'
  _globals['_TRACECONFIG_INCIDENTREPORTCONFIG'].fields_by_name['skip_dropbox']._loaded_options = None
  _globals['_TRACECONFIG_INCIDENTREPORTCONFIG'].fields_by_name['skip_dropbox']._serialized_options = b'\030\001'
  _globals['_TRACECONFIG'].fields_by_name['trace_uuid_msb']._loaded_options = None
  _globals['_TRACECONFIG'].fields_by_name['trace_uuid_msb']._serialized_options = b'\030\001'
  _globals['_TRACECONFIG'].fields_by_name['trace_uuid_lsb']._loaded_options = None
  _globals['_TRACECONFIG'].fields_by_name['trace_uuid_lsb']._serialized_options = b'\030\001'
  _globals['_TRACECONFIG']._serialized_start=156
  _globals['_TRACECONFIG']._serialized_end=4741
  _globals['_TRACECONFIG_BUFFERCONFIG']._serialized_start=1875
  _globals['_TRACECONFIG_BUFFERCONFIG']._serialized_end=2109
  _globals['_TRACECONFIG_BUFFERCONFIG_FILLPOLICY']._serialized_start=2038
  _globals['_TRACECONFIG_BUFFERCONFIG_FILLPOLICY']._serialized_end=2097
  _globals['_TRACECONFIG_DATASOURCE']._serialized_start=2112
  _globals['_TRACECONFIG_DATASOURCE']._serialized_end=2241
  _globals['_TRACECONFIG_BUILTINDATASOURCE']._serialized_start=2244
  _globals['_TRACECONFIG_BUILTINDATASOURCE']._serialized_end=2563
  _globals['_TRACECONFIG_PRODUCERCONFIG']._serialized_start=2565
  _globals['_TRACECONFIG_PRODUCERCONFIG']._serialized_end=2647
  _globals['_TRACECONFIG_STATSDMETADATA']._serialized_start=2650
  _globals['_TRACECONFIG_STATSDMETADATA']._serialized_end=2792
  _globals['_TRACECONFIG_GUARDRAILOVERRIDES']._serialized_start=2794
  _globals['_TRACECONFIG_GUARDRAILOVERRIDES']._serialized_end=2888
  _globals['_TRACECONFIG_TRIGGERCONFIG']._serialized_start=2891
  _globals['_TRACECONFIG_TRIGGERCONFIG']._serialized_end=3349
  _globals['_TRACECONFIG_TRIGGERCONFIG_TRIGGER']._serialized_start=3125
  _globals['_TRACECONFIG_TRIGGERCONFIG_TRIGGER']._serialized_end=3248
  _globals['_TRACECONFIG_TRIGGERCONFIG_TRIGGERMODE']._serialized_start=3250
  _globals['_TRACECONFIG_TRIGGERCONFIG_TRIGGERMODE']._serialized_end=3343
  _globals['_TRACECONFIG_INCREMENTALSTATECONFIG']._serialized_start=3351
  _globals['_TRACECONFIG_INCREMENTALSTATECONFIG']._serialized_end=3400
  _globals['_TRACECONFIG_INCIDENTREPORTCONFIG']._serialized_start=3403
  _globals['_TRACECONFIG_INCIDENTREPORTCONFIG']._serialized_end=3554
  _globals['_TRACECONFIG_TRACEFILTER']._serialized_start=3557
  _globals['_TRACECONFIG_TRACEFILTER']._serialized_end=4154
  _globals['_TRACECONFIG_TRACEFILTER_STRINGFILTERRULE']._serialized_start=3701
  _globals['_TRACECONFIG_TRACEFILTER_STRINGFILTERRULE']._serialized_end=3855
  _globals['_TRACECONFIG_TRACEFILTER_STRINGFILTERCHAIN']._serialized_start=3857
  _globals['_TRACECONFIG_TRACEFILTER_STRINGFILTERCHAIN']._serialized_end=3950
  _globals['_TRACECONFIG_TRACEFILTER_STRINGFILTERPOLICY']._serialized_start=3953
  _globals['_TRACECONFIG_TRACEFILTER_STRINGFILTERPOLICY']._serialized_end=4154
  _globals['_TRACECONFIG_ANDROIDREPORTCONFIG']._serialized_start=4157
  _globals['_TRACECONFIG_ANDROIDREPORTCONFIG']._serialized_end=4308
  _globals['_TRACECONFIG_CMDTRACESTARTDELAY']._serialized_start=4310
  _globals['_TRACECONFIG_CMDTRACESTARTDELAY']._serialized_end=4374
  _globals['_TRACECONFIG_SESSIONSEMAPHORE']._serialized_start=4376
  _globals['_TRACECONFIG_SESSIONSEMAPHORE']._serialized_end=4441
  _globals['_TRACECONFIG_LOCKDOWNMODEOPERATION']._serialized_start=4443
  _globals['_TRACECONFIG_LOCKDOWNMODEOPERATION']._serialized_end=4528
  _globals['_TRACECONFIG_COMPRESSIONTYPE']._serialized_start=4530
  _globals['_TRACECONFIG_COMPRESSIONTYPE']._serialized_end=4611
  _globals['_TRACECONFIG_STATSDLOGGING']._serialized_start=4613
  _globals['_TRACECONFIG_STATSDLOGGING']._serialized_end=4717
# @@protoc_insertion_point(module_scope)
