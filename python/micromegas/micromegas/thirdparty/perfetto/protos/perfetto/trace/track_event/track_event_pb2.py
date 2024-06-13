# -*- coding: utf-8 -*-
# Generated by the protocol buffer compiler.  DO NOT EDIT!
# NO CHECKED-IN PROTOBUF GENCODE
# source: protos/perfetto/trace/track_event/track_event.proto
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
    'protos/perfetto/trace/track_event/track_event.proto'
)
# @@protoc_insertion_point(imports)

_sym_db = _symbol_database.Default()


from protos.perfetto.trace.track_event import debug_annotation_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_debug__annotation__pb2
from protos.perfetto.trace.track_event import log_message_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_log__message__pb2
from protos.perfetto.trace.track_event import task_execution_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_task__execution__pb2
from protos.perfetto.trace.track_event import chrome_active_processes_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_chrome__active__processes__pb2
from protos.perfetto.trace.track_event import chrome_application_state_info_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_chrome__application__state__info__pb2
from protos.perfetto.trace.track_event import chrome_compositor_scheduler_state_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_chrome__compositor__scheduler__state__pb2
from protos.perfetto.trace.track_event import chrome_content_settings_event_info_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_chrome__content__settings__event__info__pb2
from protos.perfetto.trace.track_event import chrome_frame_reporter_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_chrome__frame__reporter__pb2
from protos.perfetto.trace.track_event import chrome_histogram_sample_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_chrome__histogram__sample__pb2
from protos.perfetto.trace.track_event import chrome_keyed_service_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_chrome__keyed__service__pb2
from protos.perfetto.trace.track_event import chrome_latency_info_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_chrome__latency__info__pb2
from protos.perfetto.trace.track_event import chrome_legacy_ipc_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_chrome__legacy__ipc__pb2
from protos.perfetto.trace.track_event import chrome_message_pump_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_chrome__message__pump__pb2
from protos.perfetto.trace.track_event import chrome_mojo_event_info_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_chrome__mojo__event__info__pb2
from protos.perfetto.trace.track_event import chrome_renderer_scheduler_state_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_chrome__renderer__scheduler__state__pb2
from protos.perfetto.trace.track_event import chrome_user_event_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_chrome__user__event__pb2
from protos.perfetto.trace.track_event import chrome_window_handle_event_info_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_chrome__window__handle__event__info__pb2
from protos.perfetto.trace.track_event import pixel_modem_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_pixel__modem__pb2
from protos.perfetto.trace.track_event import screenshot_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_screenshot__pb2
from protos.perfetto.trace.track_event import source_location_pb2 as protos_dot_perfetto_dot_trace_dot_track__event_dot_source__location__pb2


DESCRIPTOR = _descriptor_pool.Default().AddSerializedFile(b'\n3protos/perfetto/trace/track_event/track_event.proto\x12\x0fperfetto.protos\x1a\x38protos/perfetto/trace/track_event/debug_annotation.proto\x1a\x33protos/perfetto/trace/track_event/log_message.proto\x1a\x36protos/perfetto/trace/track_event/task_execution.proto\x1a?protos/perfetto/trace/track_event/chrome_active_processes.proto\x1a\x45protos/perfetto/trace/track_event/chrome_application_state_info.proto\x1aIprotos/perfetto/trace/track_event/chrome_compositor_scheduler_state.proto\x1aJprotos/perfetto/trace/track_event/chrome_content_settings_event_info.proto\x1a=protos/perfetto/trace/track_event/chrome_frame_reporter.proto\x1a?protos/perfetto/trace/track_event/chrome_histogram_sample.proto\x1a<protos/perfetto/trace/track_event/chrome_keyed_service.proto\x1a;protos/perfetto/trace/track_event/chrome_latency_info.proto\x1a\x39protos/perfetto/trace/track_event/chrome_legacy_ipc.proto\x1a;protos/perfetto/trace/track_event/chrome_message_pump.proto\x1a>protos/perfetto/trace/track_event/chrome_mojo_event_info.proto\x1aGprotos/perfetto/trace/track_event/chrome_renderer_scheduler_state.proto\x1a\x39protos/perfetto/trace/track_event/chrome_user_event.proto\x1aGprotos/perfetto/trace/track_event/chrome_window_handle_event_info.proto\x1a\x33protos/perfetto/trace/track_event/pixel_modem.proto\x1a\x32protos/perfetto/trace/track_event/screenshot.proto\x1a\x37protos/perfetto/trace/track_event/source_location.proto\"\xa7\x18\n\nTrackEvent\x12\x15\n\rcategory_iids\x18\x03 \x03(\x04\x12\x12\n\ncategories\x18\x16 \x03(\t\x12\x12\n\x08name_iid\x18\n \x01(\x04H\x00\x12\x0e\n\x04name\x18\x17 \x01(\tH\x00\x12.\n\x04type\x18\t \x01(\x0e\x32 .perfetto.protos.TrackEvent.Type\x12\x12\n\ntrack_uuid\x18\x0b \x01(\x04\x12\x17\n\rcounter_value\x18\x1e \x01(\x03H\x01\x12\x1e\n\x14\x64ouble_counter_value\x18, \x01(\x01H\x01\x12!\n\x19\x65xtra_counter_track_uuids\x18\x1f \x03(\x04\x12\x1c\n\x14\x65xtra_counter_values\x18\x0c \x03(\x03\x12(\n extra_double_counter_track_uuids\x18- \x03(\x04\x12#\n\x1b\x65xtra_double_counter_values\x18. \x03(\x01\x12\x18\n\x0c\x66low_ids_old\x18$ \x03(\x04\x42\x02\x18\x01\x12\x10\n\x08\x66low_ids\x18/ \x03(\x06\x12$\n\x18terminating_flow_ids_old\x18* \x03(\x04\x42\x02\x18\x01\x12\x1c\n\x14terminating_flow_ids\x18\x30 \x03(\x06\x12;\n\x11\x64\x65\x62ug_annotations\x18\x04 \x03(\x0b\x32 .perfetto.protos.DebugAnnotation\x12\x36\n\x0etask_execution\x18\x05 \x01(\x0b\x32\x1e.perfetto.protos.TaskExecution\x12\x30\n\x0blog_message\x18\x15 \x01(\x0b\x32\x1b.perfetto.protos.LogMessage\x12K\n\x12\x63\x63_scheduler_state\x18\x18 \x01(\x0b\x32/.perfetto.protos.ChromeCompositorSchedulerState\x12;\n\x11\x63hrome_user_event\x18\x19 \x01(\x0b\x32 .perfetto.protos.ChromeUserEvent\x12\x41\n\x14\x63hrome_keyed_service\x18\x1a \x01(\x0b\x32#.perfetto.protos.ChromeKeyedService\x12;\n\x11\x63hrome_legacy_ipc\x18\x1b \x01(\x0b\x32 .perfetto.protos.ChromeLegacyIpc\x12G\n\x17\x63hrome_histogram_sample\x18\x1c \x01(\x0b\x32&.perfetto.protos.ChromeHistogramSample\x12?\n\x13\x63hrome_latency_info\x18\x1d \x01(\x0b\x32\".perfetto.protos.ChromeLatencyInfo\x12\x43\n\x15\x63hrome_frame_reporter\x18  \x01(\x0b\x32$.perfetto.protos.ChromeFrameReporter\x12R\n\x1d\x63hrome_application_state_info\x18\' \x01(\x0b\x32+.perfetto.protos.ChromeApplicationStateInfo\x12V\n\x1f\x63hrome_renderer_scheduler_state\x18( \x01(\x0b\x32-.perfetto.protos.ChromeRendererSchedulerState\x12U\n\x1f\x63hrome_window_handle_event_info\x18) \x01(\x0b\x32,.perfetto.protos.ChromeWindowHandleEventInfo\x12[\n\"chrome_content_settings_event_info\x18+ \x01(\x0b\x32/.perfetto.protos.ChromeContentSettingsEventInfo\x12G\n\x17\x63hrome_active_processes\x18\x31 \x01(\x0b\x32&.perfetto.protos.ChromeActiveProcesses\x12/\n\nscreenshot\x18\x32 \x01(\x0b\x32\x1b.perfetto.protos.Screenshot\x12J\n\x19pixel_modem_event_insight\x18\x33 \x01(\x0b\x32\'.perfetto.protos.PixelModemEventInsight\x12:\n\x0fsource_location\x18! \x01(\x0b\x32\x1f.perfetto.protos.SourceLocationH\x02\x12\x1d\n\x13source_location_iid\x18\" \x01(\x04H\x02\x12?\n\x13\x63hrome_message_pump\x18# \x01(\x0b\x32\".perfetto.protos.ChromeMessagePump\x12\x44\n\x16\x63hrome_mojo_event_info\x18& \x01(\x0b\x32$.perfetto.protos.ChromeMojoEventInfo\x12\x1c\n\x12timestamp_delta_us\x18\x01 \x01(\x03H\x03\x12\x1f\n\x15timestamp_absolute_us\x18\x10 \x01(\x03H\x03\x12\x1e\n\x14thread_time_delta_us\x18\x02 \x01(\x03H\x04\x12!\n\x17thread_time_absolute_us\x18\x11 \x01(\x03H\x04\x12(\n\x1ethread_instruction_count_delta\x18\x08 \x01(\x03H\x05\x12+\n!thread_instruction_count_absolute\x18\x14 \x01(\x03H\x05\x12=\n\x0clegacy_event\x18\x06 \x01(\x0b\x32\'.perfetto.protos.TrackEvent.LegacyEvent\x1a\xaa\x05\n\x0bLegacyEvent\x12\x10\n\x08name_iid\x18\x01 \x01(\x04\x12\r\n\x05phase\x18\x02 \x01(\x05\x12\x13\n\x0b\x64uration_us\x18\x03 \x01(\x03\x12\x1a\n\x12thread_duration_us\x18\x04 \x01(\x03\x12 \n\x18thread_instruction_delta\x18\x0f \x01(\x03\x12\x15\n\x0bunscoped_id\x18\x06 \x01(\x04H\x00\x12\x12\n\x08local_id\x18\n \x01(\x04H\x00\x12\x13\n\tglobal_id\x18\x0b \x01(\x04H\x00\x12\x10\n\x08id_scope\x18\x07 \x01(\t\x12\x15\n\ruse_async_tts\x18\t \x01(\x08\x12\x0f\n\x07\x62ind_id\x18\x08 \x01(\x04\x12\x19\n\x11\x62ind_to_enclosing\x18\x0c \x01(\x08\x12M\n\x0e\x66low_direction\x18\r \x01(\x0e\x32\x35.perfetto.protos.TrackEvent.LegacyEvent.FlowDirection\x12V\n\x13instant_event_scope\x18\x0e \x01(\x0e\x32\x39.perfetto.protos.TrackEvent.LegacyEvent.InstantEventScope\x12\x14\n\x0cpid_override\x18\x12 \x01(\x05\x12\x14\n\x0ctid_override\x18\x13 \x01(\x05\"P\n\rFlowDirection\x12\x14\n\x10\x46LOW_UNSPECIFIED\x10\x00\x12\x0b\n\x07\x46LOW_IN\x10\x01\x12\x0c\n\x08\x46LOW_OUT\x10\x02\x12\x0e\n\nFLOW_INOUT\x10\x03\"a\n\x11InstantEventScope\x12\x15\n\x11SCOPE_UNSPECIFIED\x10\x00\x12\x10\n\x0cSCOPE_GLOBAL\x10\x01\x12\x11\n\rSCOPE_PROCESS\x10\x02\x12\x10\n\x0cSCOPE_THREAD\x10\x03\x42\x04\n\x02idJ\x04\x08\x05\x10\x06\"j\n\x04Type\x12\x14\n\x10TYPE_UNSPECIFIED\x10\x00\x12\x14\n\x10TYPE_SLICE_BEGIN\x10\x01\x12\x12\n\x0eTYPE_SLICE_END\x10\x02\x12\x10\n\x0cTYPE_INSTANT\x10\x03\x12\x10\n\x0cTYPE_COUNTER\x10\x04*\x06\x08\xe8\x07\x10\xd0\x0f*\x06\x08\xd0\x0f\x10\xd1\x0f*\x06\x08\xd1\x0f\x10\xacM*\x06\x08\xacM\x10\x91NB\x0c\n\nname_fieldB\x15\n\x13\x63ounter_value_fieldB\x17\n\x15source_location_fieldB\x0b\n\ttimestampB\r\n\x0bthread_timeB\x1a\n\x18thread_instruction_count\"u\n\x12TrackEventDefaults\x12\x12\n\ntrack_uuid\x18\x0b \x01(\x04\x12!\n\x19\x65xtra_counter_track_uuids\x18\x1f \x03(\x04\x12(\n extra_double_counter_track_uuids\x18- \x03(\x04\"*\n\rEventCategory\x12\x0b\n\x03iid\x18\x01 \x01(\x04\x12\x0c\n\x04name\x18\x02 \x01(\t\"&\n\tEventName\x12\x0b\n\x03iid\x18\x01 \x01(\x04\x12\x0c\n\x04name\x18\x02 \x01(\t')

_globals = globals()
_builder.BuildMessageAndEnumDescriptors(DESCRIPTOR, _globals)
_builder.BuildTopDescriptorsAndMessages(DESCRIPTOR, 'protos.perfetto.trace.track_event.track_event_pb2', _globals)
if not _descriptor._USE_C_DESCRIPTORS:
  DESCRIPTOR._loaded_options = None
  _globals['_TRACKEVENT'].fields_by_name['flow_ids_old']._loaded_options = None
  _globals['_TRACKEVENT'].fields_by_name['flow_ids_old']._serialized_options = b'\030\001'
  _globals['_TRACKEVENT'].fields_by_name['terminating_flow_ids_old']._loaded_options = None
  _globals['_TRACKEVENT'].fields_by_name['terminating_flow_ids_old']._serialized_options = b'\030\001'
  _globals['_TRACKEVENT']._serialized_start=1329
  _globals['_TRACKEVENT']._serialized_end=4440
  _globals['_TRACKEVENT_LEGACYEVENT']._serialized_start=3500
  _globals['_TRACKEVENT_LEGACYEVENT']._serialized_end=4182
  _globals['_TRACKEVENT_LEGACYEVENT_FLOWDIRECTION']._serialized_start=3991
  _globals['_TRACKEVENT_LEGACYEVENT_FLOWDIRECTION']._serialized_end=4071
  _globals['_TRACKEVENT_LEGACYEVENT_INSTANTEVENTSCOPE']._serialized_start=4073
  _globals['_TRACKEVENT_LEGACYEVENT_INSTANTEVENTSCOPE']._serialized_end=4170
  _globals['_TRACKEVENT_TYPE']._serialized_start=4184
  _globals['_TRACKEVENT_TYPE']._serialized_end=4290
  _globals['_TRACKEVENTDEFAULTS']._serialized_start=4442
  _globals['_TRACKEVENTDEFAULTS']._serialized_end=4559
  _globals['_EVENTCATEGORY']._serialized_start=4561
  _globals['_EVENTCATEGORY']._serialized_end=4603
  _globals['_EVENTNAME']._serialized_start=4605
  _globals['_EVENTNAME']._serialized_end=4643
# @@protoc_insertion_point(module_scope)
