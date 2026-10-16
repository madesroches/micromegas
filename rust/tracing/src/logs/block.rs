use super::{
    LogMetadata, LogMetadataRecord, LogStaticStrEvent, LogStaticStrInteropEvent, LogStringEvent,
    LogStringInteropEvent, TaggedLogInteropEvent, TaggedLogString,
};
use crate::{
    event::{EventBlock, EventStream, ExtractDeps},
    property_set::{PropertySetDependency, record_properties},
};
use micromegas_transit::{StaticStringDependency, Utf8StaticStringDependency, prelude::*};
use rustc_hash::FxHashSet;

declare_queue_struct!(
    struct LogMsgQueue<
        LogStaticStrEvent,
        LogStringEvent,
        LogStaticStrInteropEvent,
        LogStringInteropEvent,
        TaggedLogString,
        TaggedLogInteropEvent,
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
    recorded_deps: &mut FxHashSet<u64>,
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

impl ExtractDeps for LogMsgQueue {
    type DepsQueue = LogDepsQueue;

    fn extract(&self) -> Self::DepsQueue {
        let mut deps = LogDepsQueue::new(1024 * 1024);
        let mut recorded_deps = FxHashSet::default();
        let mut last_desc = 0u64;
        let mut last_set = 0u64;
        for x in self.iter() {
            match x {
                LogMsgQueueAny::LogStaticStrEvent(evt) => {
                    let desc = evt.desc as *const _ as u64;
                    if desc != last_desc {
                        last_desc = desc;
                        record_log_event_dependencies(evt.desc, &mut recorded_deps, &mut deps);
                    }
                }
                LogMsgQueueAny::LogStringEvent(evt) => {
                    let desc = evt.desc as *const _ as u64;
                    if desc != last_desc {
                        last_desc = desc;
                        record_log_event_dependencies(evt.desc, &mut recorded_deps, &mut deps);
                    }
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
                LogMsgQueueAny::TaggedLogInteropEvent(evt) => {
                    if recorded_deps.insert(evt.target.id()) {
                        deps.push(evt.target.into_dependency());
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
                LogMsgQueueAny::TaggedLogString(evt) => {
                    let desc = evt.desc as *const _ as u64;
                    if desc != last_desc {
                        last_desc = desc;
                        record_log_event_dependencies(evt.desc, &mut recorded_deps, &mut deps);
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

pub type LogBlock = EventBlock<LogMsgQueue>;
pub type LogStream = EventStream<LogBlock>;
