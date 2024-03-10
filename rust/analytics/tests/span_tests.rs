use std::{collections::HashMap, sync::Arc};

use analytics::parse_block;
use telemetry_sink::{stream_block::StreamBlock, stream_info::get_stream_info, TelemetryGuard};
use tracing::{
    event::TracingBlock,
    prelude::Verbosity,
    spans::{BeginThreadNamedSpanEvent, SpanLocation, ThreadBlock, ThreadStream},
};

#[test]
fn test_parse_span_interops() {
    let _telemetry_guard = TelemetryGuard::new();

    let mut stream = ThreadStream::new(1024, String::from("bogus_process_id"), &[], HashMap::new());
    let stream_id = stream.stream_id().to_string();

    static SPAN_LOCATION_BEGIN: SpanLocation = SpanLocation {
        lod: Verbosity::Med,
        target: "target",
        module_path: "module_path",
        file: "file",
        line: 123,
    };
    stream.get_events_mut().push(BeginThreadNamedSpanEvent {
        thread_span_location: &SPAN_LOCATION_BEGIN,
        name: "my_function".into(),
        time: 1,
    });
    static SPAN_LOCATION_END: SpanLocation = SpanLocation {
        lod: Verbosity::Med,
        target: "target",
        module_path: "module_path",
        file: "file",
        line: 456,
    };
    stream.get_events_mut().push(BeginThreadNamedSpanEvent {
        thread_span_location: &SPAN_LOCATION_END,
        name: "my_function".into(),
        time: 2,
    });

    let mut block = stream.replace_block(Arc::new(ThreadBlock::new(1024, stream_id)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin().unwrap();
    let payload: telemetry_sink::block_wire_format::BlockPayload =
        ciborium::from_reader(&encoded[..]).unwrap();

    let stream_info = get_stream_info(&stream);
    let mut nb_span_entries = 0;
    parse_block(&stream_info, &payload, |_val| {
        //if let Some((_time, _msg)) = log_entry_from_value(&val).unwrap() {
        nb_span_entries += 1;
        //}
        Ok(true)
    })
    .unwrap();
    assert_eq!(nb_span_entries, 2);
}
