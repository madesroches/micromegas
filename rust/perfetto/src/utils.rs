use crate::protos::{
    InternedData, TracePacket, TrackDescriptor, TrackEvent,
    trace_packet::OptionalTrustedPacketSequenceId,
};

pub fn new_interned_data() -> InternedData {
    InternedData {
        event_categories: vec![],
        event_names: vec![],
        debug_annotation_names: vec![],
        debug_annotation_value_type_names: vec![],
        source_locations: vec![],
        unsymbolized_source_locations: vec![],
        log_message_body: vec![],
        histogram_names: vec![],
        build_ids: vec![],
        mapping_paths: vec![],
        source_paths: vec![],
        function_names: vec![],
        profiled_frame_symbols: vec![],
        mappings: vec![],
        frames: vec![],
        callstacks: vec![],
        vulkan_memory_keys: vec![],
        graphics_contexts: vec![],
        gpu_specifications: vec![],
        kernel_symbols: vec![],
        debug_annotation_string_values: vec![],
        packet_context: vec![],
        v8_js_function_name: vec![],
        v8_js_function: vec![],
        v8_js_script: vec![],
        v8_wasm_script: vec![],
        v8_isolate: vec![],
        protolog_string_args: vec![],
        protolog_stacktrace: vec![],
        viewcapture_package_name: vec![],
        viewcapture_window_name: vec![],
        viewcapture_view_id: vec![],
        viewcapture_class_name: vec![],
    }
}

pub fn new_trace_packet() -> TracePacket {
    TracePacket {
        timestamp: None,
        timestamp_clock_id: None,
        trusted_pid: None,
        interned_data: None,
        sequence_flags: Some(2),
        incremental_state_cleared: None,
        trace_packet_defaults: None,
        previous_packet_dropped: None,
        first_packet_on_sequence: None,
        machine_id: None,
        data: None,
        optional_trusted_uid: None,
        optional_trusted_packet_sequence_id: Some(
            OptionalTrustedPacketSequenceId::TrustedPacketSequenceId(1),
        ),
    }
}

pub fn new_track_descriptor(uuid: u64) -> TrackDescriptor {
    TrackDescriptor {
        uuid: Some(uuid),
        parent_uuid: None,
        process: None,
        chrome_process: None,
        thread: None,
        chrome_thread: None,
        counter: None,
        disallow_merging_with_system_tracks: None,
        static_or_dynamic_name: None,
    }
}

#[allow(deprecated)]
pub fn new_track_event() -> TrackEvent {
    TrackEvent {
        category_iids: vec![],
        categories: vec![],
        r#type: None,
        track_uuid: None,
        extra_counter_track_uuids: vec![],
        extra_counter_values: vec![],
        extra_double_counter_track_uuids: vec![],
        extra_double_counter_values: vec![],
        flow_ids_old: vec![],
        flow_ids: vec![],
        terminating_flow_ids_old: vec![],
        terminating_flow_ids: vec![],
        debug_annotations: vec![],
        task_execution: None,
        log_message: None,
        cc_scheduler_state: None,
        chrome_user_event: None,
        chrome_keyed_service: None,
        chrome_legacy_ipc: None,
        chrome_histogram_sample: None,
        chrome_latency_info: None,
        chrome_frame_reporter: None,
        chrome_application_state_info: None,
        chrome_renderer_scheduler_state: None,
        chrome_window_handle_event_info: None,
        chrome_content_settings_event_info: None,
        chrome_active_processes: None,
        screenshot: None,
        pixel_modem_event_insight: None,
        chrome_message_pump: None,
        chrome_mojo_event_info: None,
        legacy_event: None,
        name_field: None,
        counter_value_field: None,
        source_location_field: None,
        timestamp: None,
        thread_time: None,
        thread_instruction_count: None,
    }
}
