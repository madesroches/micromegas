use super::{
    LogMetadata, LogMetadataRecord, LogStaticStrEvent, LogStaticStrInteropEvent, LogStringEvent,
    LogStringInteropEvent, TaggedLogString,
};
use crate::{
    event::{EventBlock, EventStream, ExtractDeps},
    property_set::{PropertySet, PropertySetDependency},
};
use micromegas_transit::{StaticStringDependency, Utf8StaticStringDependency, prelude::*};
use std::collections::HashSet;

declare_queue_struct!(
    struct LogMsgQueue<
        LogStaticStrEvent,
        LogStringEvent,
        LogStaticStrInteropEvent,
        LogStringInteropEvent,
        TaggedLogString,
    > {}
);

declare_queue_struct!(
    struct LogDepsQueue<
        Utf8StaticStringDependency,
        StaticStringDependency,
        LogMetadataRecord,
        PropertySetDependency,
    > {}
);

fn record_log_event_dependencies(
    log_desc: &LogMetadata,
    recorded_deps: &mut HashSet<u64>,
    deps: &mut LogDepsQueue,
) {
    let log_ptr = log_desc as *const _ as u64;
    if recorded_deps.insert(log_ptr) {
        let name = Utf8StaticStringDependency::from(log_desc.fmt_str);
        if recorded_deps.insert(name.ptr as u64) {
            deps.push(name);
        }
        let target = Utf8StaticStringDependency::from(log_desc.target);
        if recorded_deps.insert(target.ptr as u64) {
            deps.push(target);
        }
        let module_path = Utf8StaticStringDependency::from(log_desc.module_path);
        if recorded_deps.insert(module_path.ptr as u64) {
            deps.push(module_path);
        }
        let file = Utf8StaticStringDependency::from(log_desc.file);
        if recorded_deps.insert(file.ptr as u64) {
            deps.push(file);
        }
        deps.push(LogMetadataRecord {
            id: log_ptr,
            level: log_desc.level as u32,
            fmt_str: log_desc.fmt_str.as_ptr(),
            target: log_desc.target.as_ptr(),
            module_path: log_desc.module_path.as_ptr(),
            file: log_desc.file.as_ptr(),
            line: log_desc.line,
        });
    }
}

fn record_properties(
    set: &'static PropertySet,
    recorded_deps: &mut HashSet<u64>,
    deps: &mut LogDepsQueue,
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

impl ExtractDeps for LogMsgQueue {
    type DepsQueue = LogDepsQueue;

    fn extract(&self) -> Self::DepsQueue {
        let mut deps = LogDepsQueue::new(1024 * 1024);
        let mut recorded_deps = HashSet::new();
        for x in self.iter() {
            match x {
                LogMsgQueueAny::LogStaticStrEvent(evt) => {
                    record_log_event_dependencies(evt.desc, &mut recorded_deps, &mut deps);
                }
                LogMsgQueueAny::LogStringEvent(evt) => {
                    record_log_event_dependencies(evt.desc, &mut recorded_deps, &mut deps);
                }
                LogMsgQueueAny::LogStaticStrInteropEvent(evt) => {
                    if recorded_deps.insert(evt.target.id()) {
                        deps.push(Utf8StaticStringDependency::from(&evt.target));
                    }
                    if recorded_deps.insert(evt.msg.id()) {
                        deps.push(Utf8StaticStringDependency::from(&evt.msg));
                    }
                }
                LogMsgQueueAny::LogStringInteropEvent(evt) => {
                    if recorded_deps.insert(evt.target.id()) {
                        deps.push(evt.target.into_dependency());
                    }
                }
                LogMsgQueueAny::TaggedLogString(evt) => {
                    record_log_event_dependencies(evt.desc, &mut recorded_deps, &mut deps);
                    record_properties(evt.properties, &mut recorded_deps, &mut deps);
                }
            }
        }
        deps
    }
}

pub type LogBlock = EventBlock<LogMsgQueue>;
pub type LogStream = EventStream<LogBlock>;
