use super::{TaggedFloatMetricEvent, TaggedIntegerMetricEvent};
use crate::{
    event::{EventBlock, EventStream, ExtractDeps},
    metrics::{
        FloatMetricEvent, IntegerMetricEvent, MetricMetadataDependency, StaticMetricMetadata,
    },
    property_set::{PropertySetDependency, record_properties},
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
        let mut recorded_deps = FxHashSet::default();
        let mut last_desc = 0u64;
        let mut last_set = 0u64;
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
                    let set = evt.properties as u64;
                    if set != last_set {
                        last_set = set;
                        // SAFETY: the set is interned or owned by this block's arena.
                        unsafe {
                            record_properties(
                                evt.properties,
                                &mut recorded_deps,
                                &mut deps,
                                |q, d| q.push(d),
                                |q, d| q.push(d),
                            );
                        }
                    }
                }
                MetricsMsgQueueAny::TaggedIntegerMetricEvent(evt) => {
                    let desc = evt.desc as *const _ as u64;
                    if desc != last_desc {
                        last_desc = desc;
                        record_metric_event_dependencies(evt.desc, &mut recorded_deps, &mut deps);
                    }
                    let set = evt.properties as u64;
                    if set != last_set {
                        last_set = set;
                        // SAFETY: the set is interned or owned by this block's arena.
                        unsafe {
                            record_properties(
                                evt.properties,
                                &mut recorded_deps,
                                &mut deps,
                                |q, d| q.push(d),
                                |q, d| q.push(d),
                            );
                        }
                    }
                }
            }
        }
        deps
    }
}

pub type MetricsBlock = EventBlock<MetricsMsgQueue>;
pub type MetricsStream = EventStream<MetricsBlock>;
