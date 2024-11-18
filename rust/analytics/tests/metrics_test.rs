use std::{collections::HashMap, sync::Arc};

use micromegas_analytics::{measure::measure_from_value, payload::parse_block, time::ConvertTicks};
use micromegas_telemetry_sink::{
    stream_block::StreamBlock, stream_info::make_stream_info, TelemetryGuard,
};
use micromegas_tracing::{
    dispatch::make_process_info,
    event::TracingBlock,
    metrics::{
        FloatMetricEvent, IntegerMetricEvent, MetricsBlock, MetricsStream, StaticMetricMetadata,
        TaggedFloatMetricEvent, TaggedIntegerMetricEvent,
    },
    prelude::Verbosity,
    property_set::{Property, PropertySet},
    time::now,
};

#[test]
fn test_static_metrics() {
    let _telemetry_guard = TelemetryGuard::new();

    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, Some(uuid::Uuid::new_v4()));
    let mut stream = MetricsStream::new(1024, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    static METRIC_DESC: StaticMetricMetadata = StaticMetricMetadata {
        lod: Verbosity::Med,
        name: "name",
        unit: "cubits",
        target: "target",
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

    let mut block =
        stream.replace_block(Arc::new(MetricsBlock::new(1024, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info).unwrap();
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

#[test]
fn test_tagged_measures() {
    let _telemetry_guard = TelemetryGuard::new();

    let process_id = uuid::Uuid::new_v4();
    let process_info = Arc::new(make_process_info(process_id, Some(uuid::Uuid::new_v4())));
    let mut stream = MetricsStream::new(1024, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();
    stream.get_events_mut().push(TaggedIntegerMetricEvent {
        properties: PropertySet::find_or_create(vec![
            Property::new("name", "road_width"),
            Property::new("animal", "chicken"),
        ]),
        value: 2,
        time: now(),
    });
    stream.get_events_mut().push(TaggedFloatMetricEvent {
        properties: PropertySet::find_or_create(vec![
            Property::new("name", "road_width"),
            Property::new("animal", "chicken"),
        ]),
        value: 2.0,
        time: 1,
    });
    let mut block =
        stream.replace_block(Arc::new(MetricsBlock::new(1024, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info).unwrap();
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();
    let stream_info = make_stream_info(&stream);
    let convert_ticks = ConvertTicks::new(&process_info);
    let mut measures = vec![];
    parse_block(&stream_info, &received_block.payload, |val| {
        measures.push(
            measure_from_value(process_info.clone(), &convert_ticks, &val)
                .unwrap()
                .unwrap(),
        );
        Ok(true)
    })
    .unwrap();
    assert_eq!(measures.len(), 2);
}
