use std::{collections::HashMap, sync::Arc};

use micromegas_analytics::parse_block;
use micromegas_telemetry_sink::{
    stream_block::StreamBlock, stream_info::make_stream_info, TelemetryGuard,
};
use micromegas_tracing::{
    event::TracingBlock,
    metrics::{FloatMetricEvent, IntegerMetricEvent, MetricMetadata, MetricsBlock, MetricsStream},
    prelude::Verbosity,
};

#[test]
fn test_parse_metric_interops() {
    let _telemetry_guard = TelemetryGuard::new();

    let process_id = String::from("bogus_process_id");
    let mut stream = MetricsStream::new(1024, process_id.clone(), &[], HashMap::new());
    let stream_id = stream.stream_id().to_string();

    static METRIC_DESC: MetricMetadata = MetricMetadata {
        lod: Verbosity::Med,
        name: "name",
        unit: "cubits",
        target: "target",
        module_path: "module_path",
        file: "file",
        line: 123,
    };
    stream.get_events_mut().push(IntegerMetricEvent {
        desc: &METRIC_DESC,
        value: 3,
        time: 1,
    });
    stream.get_events_mut().push(FloatMetricEvent {
        desc: &METRIC_DESC,
        value: 3.0,
        time: 2,
    });

    let mut block = stream.replace_block(Arc::new(MetricsBlock::new(1024, process_id, stream_id)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin().unwrap();
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();

    let stream_info = make_stream_info(&stream);
    let mut nb_metric_entries = 0;
    parse_block(&stream_info, &received_block.payload, |_val| {
        nb_metric_entries += 1;
        Ok(true)
    })
    .unwrap();
    assert_eq!(nb_metric_entries, 2);
}
