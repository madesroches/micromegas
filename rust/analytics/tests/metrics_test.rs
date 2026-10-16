use std::{collections::HashMap, sync::Arc};

use crate::intern_string::intern_string;
use micromegas_analytics::{
    measure::measure_from_value, metadata::StreamMetadata, payload::parse_block, time::ConvertTicks,
};
use micromegas_telemetry_sink::{stream_block::StreamBlock, stream_info::make_stream_info};
use micromegas_tracing::{
    dispatch::make_process_info,
    dyn_deps::{MergedProperties, merge_properties},
    event::TracingBlock,
    intern_string,
    metrics::{
        FloatMetricEvent, IntegerMetricEvent, MetricsBlock, MetricsStream, StaticMetricMetadata,
        TaggedFloatMetricEvent, TaggedIntegerMetricEvent,
    },
    prelude::*,
    property_set::{Property, PropertySet, PropertySetId},
    test_utils::init_in_memory_tracing,
    time::now,
};
use serial_test::serial;

mod test_helpers;
use test_helpers::make_process_metadata;

#[test]
#[serial]
fn test_static_metrics() {
    let process_id = uuid::Uuid::new_v4();
    let process_info_for_encode =
        make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
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
    let encoded = block.encode_bin(&process_info_for_encode).unwrap();
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();

    let stream_info = make_stream_info(&stream);
    let stream_metadata = StreamMetadata::from_stream_info(&stream_info).unwrap();
    let mut nb_metric_entries = 0;
    parse_block(&stream_metadata, &received_block.payload, |_val| {
        nb_metric_entries += 1;
        Ok(true)
    })
    .unwrap();
    assert_eq!(nb_metric_entries, 2);

    let guard = init_in_memory_tracing();
    imetric!("test_metric", "units", 42);
    fmetric!("test_metric_float", "units", std::f64::consts::PI);
    micromegas_tracing::dispatch::flush_metrics_buffer();

    let state = guard.sink.state.lock().unwrap();
    assert!(state.process_info.is_some());
    assert!(state.metrics_stream_desc.is_some());
}

#[test]
#[serial]
fn test_stress_tagged_measures() {
    let process_id = uuid::Uuid::new_v4();
    let process_info = make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let mut stream = MetricsStream::new(1024, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    static METRIC_DESC: StaticMetricMetadata = StaticMetricMetadata {
        lod: Verbosity::Med,
        name: "static_name",
        unit: "static_unit",
        target: "static_target",
        file: "file",
        line: 123,
    };

    for i in 0..2000 {
        let value_str = intern_string(&format!("{}", i % 127));
        stream.get_events_mut().push(TaggedIntegerMetricEvent {
            desc: &METRIC_DESC,
            properties: PropertySetId::interned(PropertySet::find_or_create(vec![
                Property::new("value", value_str),
                Property::new("name", "override_name"),
                Property::new("unit", "override_unit"),
                Property::new("target", "override_target"),
            ])),
            value: 2,
            time: now(),
        });
    }
    let mut block =
        stream.replace_block(Arc::new(MetricsBlock::new(1024, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let _encoded = block.encode_bin(&process_info).unwrap();

    let guard = init_in_memory_tracing();
    for i in 0..10 {
        imetric!("stress_test", "iterations", i);
    }
    micromegas_tracing::dispatch::flush_metrics_buffer();

    let state = guard.sink.state.lock().unwrap();
    assert!(state.process_info.is_some());
    assert!(state.metrics_stream_desc.is_some());
}

#[test]
#[serial]
fn test_tagged_measures() {
    let process_id = uuid::Uuid::new_v4();
    let process_info_for_encode =
        make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let process_info = Arc::new(make_process_metadata(
        process_id,
        Some(uuid::Uuid::new_v4()),
        HashMap::new(),
    ));
    let mut stream = MetricsStream::new(1024, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    static METRIC_DESC: StaticMetricMetadata = StaticMetricMetadata {
        lod: Verbosity::Med,
        name: "static_name",
        unit: "static_unit",
        target: "static_target",
        file: "file",
        line: 123,
    };

    stream.get_events_mut().push(TaggedIntegerMetricEvent {
        desc: &METRIC_DESC,
        properties: PropertySetId::interned(PropertySet::find_or_create(vec![
            Property::new("name", "override_name"),
            Property::new("unit", "override_unit"),
            Property::new("target", "override_target"),
        ])),
        value: 2,
        time: now(),
    });
    stream.get_events_mut().push(TaggedFloatMetricEvent {
        desc: &METRIC_DESC,
        properties: PropertySetId::interned(PropertySet::find_or_create(vec![
            Property::new("name", "override_name"),
            Property::new("unit", "override_unit"),
            Property::new("target", "override_target"),
        ])),
        value: 2.0,
        time: 1,
    });
    let mut block =
        stream.replace_block(Arc::new(MetricsBlock::new(1024, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info_for_encode).unwrap();
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();
    let stream_info = make_stream_info(&stream);
    let stream_metadata = StreamMetadata::from_stream_info(&stream_info).unwrap();
    let convert_ticks = ConvertTicks::from_meta_data(0, 0, 1).unwrap();
    // Measures borrow the per-block arena, so we assert inside the callback rather
    // than collecting them (the higher-ranked closure forbids escaping the arena).
    let mut nb_measures = 0;
    parse_block(&stream_metadata, &received_block.payload, |val| {
        let measure = measure_from_value(
            process_info.clone(),
            stream_metadata.stream_id.to_string().into(),
            received_block.block_id.to_string().into(),
            0,
            &convert_ticks,
            val,
        )
        .unwrap()
        .unwrap();
        assert_eq!(measure.name, "override_name");
        assert_eq!(measure.unit, "override_unit");
        assert_eq!(measure.target, "override_target");
        nb_measures += 1;
        Ok(true)
    })
    .unwrap();
    assert_eq!(nb_measures, 2);

    let guard = init_in_memory_tracing();
    imetric!("tagged_int", "count", 100);
    fmetric!("tagged_float", "rate", 2.5);
    micromegas_tracing::dispatch::flush_metrics_buffer();

    let state = guard.sink.state.lock().unwrap();
    assert!(state.process_info.is_some());
    assert!(state.metrics_stream_desc.is_some());
}

#[test]
#[serial]
fn test_dynamic_tagged_measures() {
    let process_id = uuid::Uuid::new_v4();
    let process_info_for_encode =
        make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let process_info = Arc::new(make_process_metadata(
        process_id,
        Some(uuid::Uuid::new_v4()),
        HashMap::new(),
    ));
    let mut stream = MetricsStream::new(8192, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    static METRIC_DESC: StaticMetricMetadata = StaticMetricMetadata {
        lod: Verbosity::Med,
        name: "static_name",
        unit: "static_unit",
        target: "static_target",
        file: "file",
        line: 123,
    };
    let static_set = PropertySet::find_or_create(vec![Property::new("animal", "chicken")]);
    let correlation_id = String::from("abc");
    let merged = merge_properties(
        static_set,
        &[DynProperty::new("correlation_id", &correlation_id)],
    );
    let (events, dyn_deps) = stream.get_block_mut_parts();
    // An interned set that shares names and static values with the dynamic one goes first.
    events.push(TaggedFloatMetricEvent {
        desc: &METRIC_DESC,
        properties: PropertySetId::interned(static_set),
        value: 1.0,
        time: now(),
    });
    let properties = dyn_deps.intern_set(&merged);
    events.push(TaggedIntegerMetricEvent {
        desc: &METRIC_DESC,
        properties,
        value: 2,
        time: now(),
    });
    // same content, so it must resolve to the same wire set
    let properties = dyn_deps.intern_set(&merged);
    events.push(TaggedFloatMetricEvent {
        desc: &METRIC_DESC,
        properties,
        value: 2.0,
        time: now(),
    });

    let mut block =
        stream.replace_block(Arc::new(MetricsBlock::new(8192, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info_for_encode).unwrap();
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();
    let stream_info = make_stream_info(&stream);
    let stream_metadata = StreamMetadata::from_stream_info(&stream_info).unwrap();
    let convert_ticks = ConvertTicks::from_meta_data(0, 0, 1).unwrap();
    let mut set_ptrs = vec![];
    parse_block(&stream_metadata, &received_block.payload, |val| {
        let measure = measure_from_value(
            process_info.clone(),
            stream_metadata.stream_id.to_string().into(),
            received_block.block_id.to_string().into(),
            0,
            &convert_ticks,
            val,
        )
        .unwrap()
        .unwrap();
        let mut props = vec![];
        measure
            .properties
            .for_each_property(|k, v| {
                props.push((k.to_owned(), v.to_owned()));
                Ok(())
            })
            .unwrap();
        assert!(props.contains(&("animal".to_owned(), "chicken".to_owned())));
        if measure.value == 1.0 {
            assert_eq!(props.len(), 1);
        } else {
            assert!(props.contains(&("correlation_id".to_owned(), "abc".to_owned())));
        }
        set_ptrs.push(measure.properties.object_ptr() as usize);
        Ok(true)
    })
    .unwrap();
    assert_eq!(set_ptrs.len(), 3);
    assert_ne!(set_ptrs[0], set_ptrs[1]);
    assert_eq!(set_ptrs[1], set_ptrs[2]);
}

fn name_and_unit<'a>(
    name: &'a str,
    unit: &'a str,
    extra: Option<DynProperty<'a>>,
) -> MergedProperties<'a> {
    let mut props: Vec<DynProperty<'a>> = extra.into_iter().collect();
    props.push(DynProperty::new("name", name));
    props.push(DynProperty::new("unit", unit));
    merge_properties(PropertySet::empty(), &props)
}

#[test]
#[serial]
fn test_dynamic_name_measures() {
    let process_id = uuid::Uuid::new_v4();
    let process_info_for_encode =
        make_process_info(process_id, Some(uuid::Uuid::new_v4()), HashMap::new());
    let process_info = Arc::new(make_process_metadata(
        process_id,
        Some(uuid::Uuid::new_v4()),
        HashMap::new(),
    ));
    let mut stream = MetricsStream::new(8192, process_id, &[], HashMap::new());
    let stream_id = stream.stream_id();

    static STATIC_DESC: StaticMetricMetadata = StaticMetricMetadata {
        lod: Verbosity::Med,
        name: "same_name",
        unit: "count",
        target: "static_target",
        file: "file",
        line: 123,
    };
    static DYN_DESC: StaticMetricMetadata = StaticMetricMetadata {
        lod: Verbosity::Med,
        name: "",
        unit: "",
        target: "dyn_target",
        file: "file",
        line: 456,
    };
    let (events, dyn_deps) = stream.get_block_mut_parts();
    events.push(IntegerMetricEvent {
        desc: &STATIC_DESC,
        value: 1,
        time: now(),
    });
    // a dynamic name equal to a static metric's name
    let properties = dyn_deps.intern_set(&name_and_unit("same_name", "count", None));
    events.push(TaggedIntegerMetricEvent {
        desc: &DYN_DESC,
        properties,
        value: 2,
        time: now(),
    });
    // two names sharing a unit
    let properties = dyn_deps.intern_set(&name_and_unit("other_name", "count", None));
    events.push(TaggedFloatMetricEvent {
        desc: &DYN_DESC,
        properties,
        value: 3.0,
        time: now(),
    });
    // a dynamic ticks unit
    let properties = dyn_deps.intern_set(&name_and_unit("elapsed", "ticks", None));
    events.push(TaggedIntegerMetricEvent {
        desc: &DYN_DESC,
        properties,
        value: 5,
        time: now(),
    });
    // a dynamic property name
    let key = String::from("region_key");
    let properties = dyn_deps.intern_set(&name_and_unit(
        "with_key",
        "count",
        Some(DynProperty::with_dyn_name(&key, "eu")),
    ));
    events.push(TaggedFloatMetricEvent {
        desc: &DYN_DESC,
        properties,
        value: 6.0,
        time: now(),
    });

    let mut block =
        stream.replace_block(Arc::new(MetricsBlock::new(8192, process_id, stream_id, 0)));
    Arc::get_mut(&mut block).unwrap().close();
    let encoded = block.encode_bin(&process_info_for_encode).unwrap();
    let received_block: micromegas_telemetry::block_wire_format::Block =
        ciborium::from_reader(&encoded[..]).unwrap();
    let stream_info = make_stream_info(&stream);
    let stream_metadata = StreamMetadata::from_stream_info(&stream_info).unwrap();
    let convert_ticks = ConvertTicks::from_meta_data(0, 0, 1).unwrap();
    let mut seen = vec![];
    parse_block(&stream_metadata, &received_block.payload, |val| {
        let measure = measure_from_value(
            process_info.clone(),
            stream_metadata.stream_id.to_string().into(),
            received_block.block_id.to_string().into(),
            0,
            &convert_ticks,
            val,
        )
        .unwrap()
        .unwrap();
        let mut props = vec![];
        measure
            .properties
            .for_each_property(|k, v| {
                props.push((k.to_owned(), v.to_owned()));
                Ok(())
            })
            .unwrap();
        props.sort();
        seen.push((
            measure.target.to_owned(),
            measure.name.to_owned(),
            measure.unit.to_owned(),
            measure.value,
            props,
        ));
        Ok(true)
    })
    .unwrap();

    let named = |name: &str, unit: &str| {
        vec![
            ("name".to_owned(), name.to_owned()),
            ("unit".to_owned(), unit.to_owned()),
        ]
    };
    let mut with_key = named("with_key", "count");
    with_key.push(("region_key".to_owned(), "eu".to_owned()));
    with_key.sort();
    assert_eq!(
        seen,
        vec![
            (
                "static_target".to_owned(),
                "same_name".to_owned(),
                "count".to_owned(),
                1.0,
                vec![]
            ),
            (
                "dyn_target".to_owned(),
                "same_name".to_owned(),
                "count".to_owned(),
                2.0,
                named("same_name", "count")
            ),
            (
                "dyn_target".to_owned(),
                "other_name".to_owned(),
                "count".to_owned(),
                3.0,
                named("other_name", "count")
            ),
            (
                "dyn_target".to_owned(),
                "elapsed".to_owned(),
                "seconds".to_owned(),
                5.0,
                named("elapsed", "ticks")
            ),
            (
                "dyn_target".to_owned(),
                "with_key".to_owned(),
                "count".to_owned(),
                6.0,
                with_key
            ),
        ]
    );
}
