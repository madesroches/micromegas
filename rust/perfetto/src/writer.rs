use std::collections::HashMap;

use crate::protos::{
    EventCategory, EventName, InternedData, ProcessDescriptor, SourceLocation, ThreadDescriptor,
    Trace, TracePacket, TrackDescriptor, TrackEvent,
    trace_packet::{Data, OptionalTrustedPacketSequenceId},
    track_event::{self, NameField, SourceLocationField},
};
use xxhash_rust::xxh64::xxh64;

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

/// A writer for Perfetto traces.
pub struct Writer {
    trace: Trace,
    pid: i32,          // derived from micromegas's process_id using a hash function
    process_uuid: u64, // derived from micromegas's process_id using a hash function
    current_thread_uuid: Option<u64>,
    names: HashMap<String, u64>,
    categories: HashMap<String, u64>,
    source_locations: HashMap<(String, u32), u64>,
}

impl Writer {
    /// Creates a new `Writer` instance.
    pub fn new(micromegas_process_id: &str) -> Self {
        let trace = Trace { packet: vec![] };
        let process_uuid = xxh64(micromegas_process_id.as_bytes(), 0);
        let pid = process_uuid as i32;
        Self {
            trace,
            pid,
            process_uuid,
            current_thread_uuid: None,
            names: HashMap::new(),
            categories: HashMap::new(),
            source_locations: HashMap::new(),
        }
    }

    /// Appends a process descriptor to the trace.
    pub fn append_process_descriptor(&mut self, exe: &str) {
        let mut process_track = new_track_descriptor(self.process_uuid);
        process_track.process = Some(ProcessDescriptor {
            pid: Some(self.pid),
            cmdline: vec![],
            process_name: Some(exe.into()),
            process_priority: None,
            start_timestamp_ns: None,
            chrome_process_type: None,
            legacy_sort_index: None,
            process_labels: vec![],
        });
        let mut packet = new_trace_packet();
        packet.data = Some(Data::TrackDescriptor(process_track));
        packet.first_packet_on_sequence = Some(true);
        packet.sequence_flags = Some(3);
        self.trace.packet.push(packet);
    }

    /// Appends a thread descriptor to the trace.
    pub fn append_thread_descriptor(&mut self, stream_id: &str, thread_id: i32, thread_name: &str) {
        let mut packet = new_trace_packet();
        let thread_uuid = xxh64(stream_id.as_bytes(), 0);
        self.current_thread_uuid = Some(thread_uuid);
        let mut thread_track = new_track_descriptor(thread_uuid);
        thread_track.parent_uuid = Some(self.process_uuid);
        thread_track.thread = Some(ThreadDescriptor {
            pid: Some(self.pid),
            tid: Some(thread_id),
            thread_name: Some(thread_name.into()),
            chrome_thread_type: None,
            reference_timestamp_us: None,
            reference_thread_time_us: None,
            reference_thread_instruction_count: None,
            legacy_sort_index: None,
        });
        packet.data = Some(Data::TrackDescriptor(thread_track));
        self.trace.packet.push(packet);
    }

    fn set_name(&mut self, name: &str, packet: &mut TracePacket, event: &mut TrackEvent) {
        if let Some(id) = self.names.get(name) {
            event.name_field = Some(NameField::NameIid(*id));
        } else {
            let id = self.names.len() as u64 + 1;
            self.names.insert(name.to_owned(), id);
            event.name_field = Some(NameField::NameIid(id));
            if packet.interned_data.is_none() {
                packet.interned_data = Some(new_interned_data());
            }
            packet
                .interned_data
                .as_mut()
                .unwrap()
                .event_names
                .push(EventName {
                    iid: Some(id),
                    name: Some(name.to_owned()),
                });
        }
    }

    fn set_category(&mut self, category: &str, packet: &mut TracePacket, event: &mut TrackEvent) {
        if let Some(id) = self.categories.get(category) {
            event.category_iids.push(*id);
        } else {
            let id = self.categories.len() as u64 + 1;
            self.categories.insert(category.to_owned(), id);
            event.category_iids.push(id);
            if packet.interned_data.is_none() {
                packet.interned_data = Some(new_interned_data());
            }
            packet
                .interned_data
                .as_mut()
                .unwrap()
                .event_categories
                .push(EventCategory {
                    iid: Some(id),
                    name: Some(category.to_owned()),
                });
        }
    }

    fn set_source_location(
        &mut self,
        filename: &str,
        line: u32,
        packet: &mut TracePacket,
        event: &mut TrackEvent,
    ) {
        if let Some(id) = self.source_locations.get(&(filename.to_owned(), line)) {
            event.source_location_field = Some(SourceLocationField::SourceLocationIid(*id));
        } else {
            let id = self.source_locations.len() as u64 + 1;
            self.source_locations
                .insert((filename.to_owned(), line), id);
            event.source_location_field = Some(SourceLocationField::SourceLocationIid(id));
            if packet.interned_data.is_none() {
                packet.interned_data = Some(new_interned_data());
            }
            packet
                .interned_data
                .as_mut()
                .unwrap()
                .source_locations
                .push(SourceLocation {
                    iid: Some(id),
                    file_name: Some(filename.to_owned()),
                    function_name: None,
                    line_number: Some(line),
                });
        }
    }

    fn init_span_event(
        &mut self,
        name: &str,
        target: &str,
        filename: &str,
        line: u32,
        packet: &mut TracePacket,
        mut track_event: TrackEvent,
    ) {
        assert!(self.current_thread_uuid.is_some());
        track_event.track_uuid = self.current_thread_uuid;
        self.set_name(name, packet, &mut track_event);
        self.set_category(target, packet, &mut track_event);
        self.set_source_location(filename, line, packet, &mut track_event);
        packet.data = Some(Data::TrackEvent(track_event));
    }

    /// Appends a span event to the trace.
    pub fn append_span(
        &mut self,
        begin_ns: u64,
        end_ns: u64,
        name: &str,
        target: &str,
        filename: &str,
        line: u32,
    ) {
        let mut packet = new_trace_packet();
        packet.timestamp = Some(begin_ns);
        let mut track_event = new_track_event();
        track_event.r#type = Some(track_event::Type::SliceBegin.into());
        self.init_span_event(name, target, filename, line, &mut packet, track_event);
        self.trace.packet.push(packet);

        let mut packet = new_trace_packet();
        packet.timestamp = Some(end_ns);
        let mut track_event = new_track_event();
        track_event.r#type = Some(track_event::Type::SliceEnd.into());
        self.init_span_event(name, target, filename, line, &mut packet, track_event);
        self.trace.packet.push(packet);
    }

    /// Converts the `Writer` into a `Trace`.
    pub fn into_trace(self) -> Trace {
        self.trace
    }
}
