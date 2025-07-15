use super::{TaggedFloatMetricEvent, TaggedIntegerMetricEvent};
use crate::{
    event::{EventBlock, EventStream, ExtractDeps},
    metrics::{
        FloatMetricEvent, IntegerMetricEvent, MetricMetadataDependency, StaticMetricMetadata,
    },
    property_set::{PropertySet, PropertySetDependency},
};
use micromegas_transit::{StaticStringDependency, Utf8StaticStringDependency, prelude::*};
use std::collections::HashSet;

declare_queue_struct!(
    struct MetricsMsgQueue<
        IntegerMetricEvent,
        FloatMetricEvent,
        TaggedFloatMetricEvent,
        TaggedIntegerMetricEvent,
    > {}
);

declare_queue_struct!(
    struct MetricsDepsQueue<
        Utf8StaticStringDependency,
        StaticStringDependency,
        MetricMetadataDependency,
        PropertySetDependency,
    > {}
);

fn record_metric_event_dependencies(
    metric_desc: &StaticMetricMetadata,
    recorded_deps: &mut HashSet<u64>,
    deps: &mut MetricsDepsQueue,
) {
    let metric_ptr = metric_desc as *const _ as u64;
    if recorded_deps.insert(metric_ptr) {
        let name = Utf8StaticStringDependency::from(metric_desc.name);
        if recorded_deps.insert(name.ptr as u64) {
            deps.push(name);
        }
        let unit = Utf8StaticStringDependency::from(metric_desc.unit);
        if recorded_deps.insert(unit.ptr as u64) {
            deps.push(unit);
        }
        let target = Utf8StaticStringDependency::from(metric_desc.target);
        if recorded_deps.insert(target.ptr as u64) {
            deps.push(target);
        }
        let file = Utf8StaticStringDependency::from(metric_desc.file);
        if recorded_deps.insert(file.ptr as u64) {
            deps.push(file);
        }
        deps.push(MetricMetadataDependency {
            id: metric_ptr,
            name: metric_desc.name.as_ptr(),
            unit: metric_desc.unit.as_ptr(),
            target: metric_desc.target.as_ptr(),
            file: metric_desc.file.as_ptr(),
            line: metric_desc.line,
            lod: metric_desc.lod as u32,
        });
    }
}

fn record_properties(
    set: &'static PropertySet,
    recorded_deps: &mut HashSet<u64>,
    deps: &mut MetricsDepsQueue,
) {
    let id = set as *const _ as u64;
    if recorded_deps.insert(id) {
        for prop in set.get_properties() {
            if recorded_deps.insert(prop.name.id()) {
                deps.push(prop.name.into_dependency());
            }
            if recorded_deps.insert(prop.value.id()) {
                deps.push(prop.value.into_dependency());
            }
        }
        deps.push(PropertySetDependency::new(set));
    }
}

impl ExtractDeps for MetricsMsgQueue {
    type DepsQueue = MetricsDepsQueue;

    fn extract(&self) -> Self::DepsQueue {
        let mut deps = MetricsDepsQueue::new(1024 * 1024);
        let mut recorded_deps = HashSet::new();
        for x in self.iter() {
            match x {
                MetricsMsgQueueAny::IntegerMetricEvent(evt) => {
                    record_metric_event_dependencies(evt.desc, &mut recorded_deps, &mut deps);
                }
                MetricsMsgQueueAny::FloatMetricEvent(evt) => {
                    record_metric_event_dependencies(evt.desc, &mut recorded_deps, &mut deps);
                }
                MetricsMsgQueueAny::TaggedFloatMetricEvent(evt) => {
                    record_metric_event_dependencies(evt.desc, &mut recorded_deps, &mut deps);
                    record_properties(evt.properties, &mut recorded_deps, &mut deps);
                }
                MetricsMsgQueueAny::TaggedIntegerMetricEvent(evt) => {
                    record_metric_event_dependencies(evt.desc, &mut recorded_deps, &mut deps);
                    record_properties(evt.properties, &mut recorded_deps, &mut deps);
                }
            }
        }
        deps
    }
}

pub type MetricsBlock = EventBlock<MetricsMsgQueue>;
pub type MetricsStream = EventStream<MetricsBlock>;
