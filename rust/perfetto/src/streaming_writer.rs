use std::collections::HashMap;

use crate::async_writer::AsyncWriter;
use crate::protos::{
    EventCategory, EventName, ProcessDescriptor, SourceLocation, ThreadDescriptor, TracePacket,
    TrackEvent,
    trace_packet::Data,
    track_event::{self, NameField, SourceLocationField},
};

use crate::utils::{new_interned_data, new_trace_packet, new_track_descriptor, new_track_event};
use prost::{
    Message,
    encoding::{WireType, encode_key, encode_varint},
};
use xxhash_rust::xxh64::xxh64;

// Protobuf field numbers for Trace message
// Note: This corresponds to the "packet" field in the Trace message:
//   #[prost(message, repeated, tag = "1")]
//   pub packet: ::prost::alloc::vec::Vec<TracePacket>,
// Prost doesn't generate field number constants by default, so we define it manually.
// This is unlikely to change as it would break protobuf compatibility.
const TRACE_PACKET_FIELD_NUMBER: u32 = 1;

/// A writer for Perfetto traces that writes packets through an AsyncWriter.
/// Uses the AsyncWriter trait to abstract the underlying data sink.
pub struct PerfettoWriter {
    writer: Box<dyn AsyncWriter + Send>,
    pid: i32,          // derived from micromegas's process_id using a hash function
    process_uuid: u64, // derived from micromegas's process_id using a hash function
    current_thread_uuid: Option<u64>,
    async_track_uuid: Option<u64>, // Single async track UUID for all async spans
    names: HashMap<String, u64>,
    categories: HashMap<String, u64>,
    source_locations: HashMap<(String, u32), u64>,
}

impl PerfettoWriter {
    /// Creates a new `PerfettoWriter` instance.
    pub fn new(writer: Box<dyn AsyncWriter + Send>, micromegas_process_id: &str) -> Self {
        let process_uuid = xxh64(micromegas_process_id.as_bytes(), 0);
        let pid = process_uuid as i32;
        Self {
            writer,
            pid,
            process_uuid,
            current_thread_uuid: None,
            async_track_uuid: None,
            names: HashMap::new(),
            categories: HashMap::new(),
            source_locations: HashMap::new(),
        }
    }

    /// Writes a single TracePacket to the chunk sender with proper protobuf framing.
    pub async fn write_packet(&mut self, packet: TracePacket) -> anyhow::Result<()> {
        let mut packet_buf = Vec::new();
        // Encode the packet to get its bytes
        packet.encode(&mut packet_buf)?;

        let mut framing_buf = Vec::new();
        // Use prost's encoding functions to write the field tag and length
        encode_key(
            TRACE_PACKET_FIELD_NUMBER,
            WireType::LengthDelimited,
            &mut framing_buf,
        );
        encode_varint(packet_buf.len() as u64, &mut framing_buf);

        // Write the framing and packet data to the writer
        self.writer.write(&framing_buf).await?;
        self.writer.write(&packet_buf).await?;

        Ok(())
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
        file: &str,
        line: u32,
        packet: &mut TracePacket,
        event: &mut TrackEvent,
    ) {
        let key = (file.to_string(), line);
        if let Some(id) = self.source_locations.get(&key) {
            event.source_location_field = Some(SourceLocationField::SourceLocationIid(*id));
        } else {
            let id = self.source_locations.len() as u64 + 1;
            self.source_locations.insert(key, id);
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
                    file_name: Some(file.to_owned()),
                    line_number: Some(line),
                    function_name: None,
                });
        }
    }

    /// Emits a process descriptor packet to the stream.
    pub async fn emit_process_descriptor(&mut self, exe: &str) -> anyhow::Result<()> {
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
        self.write_packet(packet).await
    }

    /// Emits a thread descriptor packet to the stream.
    pub async fn emit_thread_descriptor(
        &mut self,
        stream_id: &str,
        thread_id: i32,
        thread_name: &str,
    ) -> anyhow::Result<()> {
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
        let mut packet = new_trace_packet();
        packet.data = Some(Data::TrackDescriptor(thread_track));
        self.write_packet(packet).await
    }

    /// Sets the current thread for subsequent span emissions.
    /// Must be called before emitting spans for a specific thread.
    pub fn set_current_thread(&mut self, stream_id: &str) {
        let thread_uuid = xxh64(stream_id.as_bytes(), 0);
        self.current_thread_uuid = Some(thread_uuid);
    }

    /// Emits an async track descriptor packet to the stream (single track for all async spans).
    pub async fn emit_async_track_descriptor(&mut self) -> anyhow::Result<()> {
        if self.async_track_uuid.is_some() {
            return Ok(()); // Already created
        }

        let async_track_uuid = xxh64("async_track".as_bytes(), self.process_uuid);
        self.async_track_uuid = Some(async_track_uuid);

        let mut async_track = new_track_descriptor(async_track_uuid);
        async_track.parent_uuid = Some(self.process_uuid);
        async_track.static_or_dynamic_name =
            Some(crate::protos::track_descriptor::StaticOrDynamicName::Name(
                "Async Operations".to_owned(),
            ));

        let mut packet = new_trace_packet();
        packet.data = Some(Data::TrackDescriptor(async_track));
        self.write_packet(packet).await
    }

    /// Initialize span event fields for thread spans
    fn init_span_event(
        &mut self,
        name: &str,
        target: &str,
        filename: &str,
        line: u32,
        packet: &mut TracePacket,
        mut track_event: TrackEvent,
    ) {
        track_event.track_uuid = self.current_thread_uuid;
        self.set_name(name, packet, &mut track_event);
        self.set_category(target, packet, &mut track_event);
        self.set_source_location(filename, line, packet, &mut track_event);
        packet.data = Some(Data::TrackEvent(track_event));
    }

    /// Initialize async span event fields
    fn init_async_span_event(
        &mut self,
        name: &str,
        target: &str,
        filename: &str,
        line: u32,
        packet: &mut TracePacket,
        mut track_event: TrackEvent,
    ) {
        assert!(
            self.async_track_uuid.is_some(),
            "Must call emit_async_track_descriptor() before emitting async span events"
        );

        track_event.track_uuid = self.async_track_uuid;
        self.set_name(name, packet, &mut track_event);
        self.set_category(target, packet, &mut track_event);
        self.set_source_location(filename, line, packet, &mut track_event);
        packet.data = Some(Data::TrackEvent(track_event));
    }

    /// Emits a span event to the stream.
    pub async fn emit_span(
        &mut self,
        begin_ns: u64,
        end_ns: u64,
        name: &str,
        target: &str,
        filename: &str,
        line: u32,
    ) -> anyhow::Result<()> {
        // Emit begin event
        let mut packet = new_trace_packet();
        packet.timestamp = Some(begin_ns);
        let mut track_event = new_track_event();
        track_event.r#type = Some(track_event::Type::SliceBegin.into());
        self.init_span_event(name, target, filename, line, &mut packet, track_event);
        self.write_packet(packet).await?;

        // Emit end event
        let mut packet = new_trace_packet();
        packet.timestamp = Some(end_ns);
        let mut track_event = new_track_event();
        track_event.r#type = Some(track_event::Type::SliceEnd.into());
        self.init_span_event(name, target, filename, line, &mut packet, track_event);
        self.write_packet(packet).await?;

        Ok(())
    }

    /// Emits an async span begin event to the stream.
    pub async fn emit_async_span_begin(
        &mut self,
        timestamp_ns: u64,
        name: &str,
        target: &str,
        filename: &str,
        line: u32,
    ) -> anyhow::Result<()> {
        let mut packet = new_trace_packet();
        packet.timestamp = Some(timestamp_ns);
        let mut track_event = new_track_event();
        track_event.r#type = Some(track_event::Type::SliceBegin.into());
        self.init_async_span_event(name, target, filename, line, &mut packet, track_event);
        self.write_packet(packet).await
    }

    /// Emits an async span end event to the stream.
    pub async fn emit_async_span_end(
        &mut self,
        timestamp_ns: u64,
        name: &str,
        target: &str,
        filename: &str,
        line: u32,
    ) -> anyhow::Result<()> {
        let mut packet = new_trace_packet();
        packet.timestamp = Some(timestamp_ns);
        let mut track_event = new_track_event();
        track_event.r#type = Some(track_event::Type::SliceEnd.into());
        self.init_async_span_event(name, target, filename, line, &mut packet, track_event);
        self.write_packet(packet).await
    }

    /// Flushes any buffered data in the writer.
    pub async fn flush(&mut self) -> anyhow::Result<()> {
        self.writer.flush().await
    }

    /// Consumes the writer and returns the underlying AsyncWriter.
    /// This is useful for testing to extract the written data.
    pub fn into_inner(self) -> Box<dyn AsyncWriter + Send> {
        self.writer
    }
}
