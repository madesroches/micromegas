use crate::{
    event::{EventBlock, EventStream, ExtractDeps},
    metrics::{FloatMetricEvent, IntegerMetricEvent, MetricMetadata, MetricMetadataDependency},
};
use micromegas_transit::{prelude::*, Utf8StaticStringDependency};
use std::collections::HashSet;

declare_queue_struct!(
    struct MetricsMsgQueue<IntegerMetricEvent, FloatMetricEvent> {}
);

declare_queue_struct!(
    struct MetricsDepsQueue<Utf8StaticStringDependency, MetricMetadataDependency> {}
);

fn record_metric_event_dependencies(
    metric_desc: &MetricMetadata,
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
            }
        }
        deps
    }
}

pub type MetricsBlock = EventBlock<MetricsMsgQueue>;
pub type MetricsStream = EventStream<MetricsBlock>;
