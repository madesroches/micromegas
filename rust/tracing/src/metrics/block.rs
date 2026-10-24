use super::{TaggedFloatMetricEvent, TaggedIntegerMetricEvent};
use crate::{
    dyn_deps::DynDeps,
    event::{EventBlock, EventStream, ExtractDeps},
    metrics::{
        FloatMetricEvent, IntegerMetricEvent, MetricMetadataDependency, StaticMetricMetadata,
    },
    property_set::{
        PropertyDepsQueue, PropertySetDependency, PropertySetId, StringDepsQueue, record_properties,
    },
    source_path::trim_source_path,
};
use micromegas_transit::{StaticStringDependency, Utf8StaticStringDependency, prelude::*};
use rustc_hash::FxHashSet;

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
    recorded_deps: &mut FxHashSet<u64>,
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
        let file = Utf8StaticStringDependency::from(trim_source_path(metric_desc.file));
        let file_ptr = file.ptr;
        if recorded_deps.insert(file.ptr as u64) {
            deps.push(file);
        }
        deps.push(MetricMetadataDependency {
            id: metric_ptr,
            name: metric_desc.name.as_ptr(),
            unit: metric_desc.unit.as_ptr(),
            target: metric_desc.target.as_ptr(),
            file: file_ptr,
            line: metric_desc.line,
            lod: metric_desc.lod as u32,
        });
    }
}

impl ExtractDeps for MetricsMsgQueue {
    type DepsQueue = MetricsDepsQueue;

    fn extract(&self, dyn_deps: &DynDeps<Self::DepsQueue>) -> Self::DepsQueue {
        let mut deps = MetricsDepsQueue::new(1024 * 1024);
        let mut recorded_deps = FxHashSet::default();
        let mut last_desc = 0u64;
        let mut last_set = PropertySetId(0);
        for x in self.iter() {
            match x {
                MetricsMsgQueueAny::IntegerMetricEvent(evt) => {
                    let desc = evt.desc as *const _ as u64;
                    if desc != last_desc {
                        last_desc = desc;
                        record_metric_event_dependencies(evt.desc, &mut recorded_deps, &mut deps);
                    }
                }
                MetricsMsgQueueAny::FloatMetricEvent(evt) => {
                    let desc = evt.desc as *const _ as u64;
                    if desc != last_desc {
                        last_desc = desc;
                        record_metric_event_dependencies(evt.desc, &mut recorded_deps, &mut deps);
                    }
                }
                MetricsMsgQueueAny::TaggedFloatMetricEvent(evt) => {
                    let desc = evt.desc as *const _ as u64;
                    if desc != last_desc {
                        last_desc = desc;
                        record_metric_event_dependencies(evt.desc, &mut recorded_deps, &mut deps);
                    }
                    if evt.properties != last_set {
                        last_set = evt.properties;
                        if let Some(set) = evt.properties.as_interned() {
                            record_properties(set, &mut recorded_deps, &mut deps);
                        }
                    }
                }
                MetricsMsgQueueAny::TaggedIntegerMetricEvent(evt) => {
                    let desc = evt.desc as *const _ as u64;
                    if desc != last_desc {
                        last_desc = desc;
                        record_metric_event_dependencies(evt.desc, &mut recorded_deps, &mut deps);
                    }
                    if evt.properties != last_set {
                        last_set = evt.properties;
                        if let Some(set) = evt.properties.as_interned() {
                            record_properties(set, &mut recorded_deps, &mut deps);
                        }
                    }
                }
            }
        }
        dyn_deps.record_static_refs(&mut recorded_deps, &mut deps);
        deps.extend_from(dyn_deps.queue());
        deps
    }
}

impl StringDepsQueue for MetricsDepsQueue {
    fn push_string(&mut self, dep: StaticStringDependency) {
        self.push(dep);
    }
}

impl PropertyDepsQueue for MetricsDepsQueue {
    fn push_set(&mut self, dep: PropertySetDependency) {
        self.push(dep);
    }
}

pub type MetricsBlock = EventBlock<MetricsMsgQueue>;
pub type MetricsStream = EventStream<MetricsBlock>;
