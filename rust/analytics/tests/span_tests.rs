use std::{collections::HashMap, sync::Arc};

use micromegas_analytics::parse_block;
use micromegas_telemetry_sink::{
    stream_block::StreamBlock, stream_info::make_stream_info, TelemetryGuard,
};
use micromegas_tracing::{
    dispatch::make_process_info,
    event::TracingBlock,
    prelude::Verbosity,
    spans::{BeginThreadNamedSpanEvent, SpanLocation, ThreadBlock, ThreadStream},
};

#[test]
fn test_parse_span_interops() {
    let _telemetry_guard = TelemetryGuard::new();

    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, Some(uuid::Uuid::new_v4()));
    let mut stream = ThreadStream::new(1024, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

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

    let mut block =
        stream.replace_block(Arc::new(ThreadBlock::new(1024, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info).unwrap();
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();

    let stream_info = make_stream_info(&stream);
    let mut nb_span_entries = 0;
    parse_block(&stream_info, &received_block.payload, |_val| {
        nb_span_entries += 1;
        Ok(true)
    })
    .unwrap();
    assert_eq!(nb_span_entries, 2);
}
