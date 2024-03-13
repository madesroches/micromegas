use super::{
    BeginAsyncNamedSpanEvent, BeginAsyncSpanEvent, BeginThreadNamedSpanEvent, BeginThreadSpanEvent,
    EndAsyncNamedSpanEvent, EndAsyncSpanEvent, EndThreadNamedSpanEvent, EndThreadSpanEvent,
    SpanLocation, SpanLocationRecord, SpanMetadata, SpanRecord,
};
use crate::{
    event::{EventBlock, EventStream, ExtractDeps},
    string_id::StringId,
};
use micromegas_transit::prelude::*;
use std::collections::HashSet;

declare_queue_struct!(
    struct ThreadEventQueue<
        BeginThreadSpanEvent,
        EndThreadSpanEvent,
        BeginThreadNamedSpanEvent,
        EndThreadNamedSpanEvent,
        BeginAsyncSpanEvent,
        EndAsyncSpanEvent,
        BeginAsyncNamedSpanEvent,
        EndAsyncNamedSpanEvent,
    > {}
);

declare_queue_struct!(
    struct ThreadDepsQueue<SpanRecord, SpanLocationRecord, StaticString> {}
);

fn record_scope_event_dependencies(
    thread_span_desc: &'static SpanMetadata,
    recorded_deps: &mut HashSet<u64>,
    deps: &mut ThreadDepsQueue,
) {
    let thread_span_ptr = thread_span_desc as *const _ as u64;
    if recorded_deps.insert(thread_span_ptr) {
        let name = StaticString::from(thread_span_desc.name);
        if recorded_deps.insert(name.ptr as u64) {
            deps.push(name);
        }
        let target = StaticString::from(thread_span_desc.location.target);
        if recorded_deps.insert(target.ptr as u64) {
            deps.push(target);
        }
        let module_path = StaticString::from(thread_span_desc.location.module_path);
        if recorded_deps.insert(module_path.ptr as u64) {
            deps.push(module_path);
        }
        let file = StaticString::from(thread_span_desc.location.file);
        if recorded_deps.insert(file.ptr as u64) {
            deps.push(file);
        }
        deps.push(SpanRecord {
            id: thread_span_ptr,
            name: thread_span_desc.name.as_ptr(),
            target: thread_span_desc.location.target.as_ptr(),
            module_path: thread_span_desc.location.module_path.as_ptr(),
            file: thread_span_desc.location.file.as_ptr(),
            line: thread_span_desc.location.line,
            lod: thread_span_desc.location.lod as u32,
        });
    }
}

fn record_named_scope_event_dependencies(
    thread_span_location: &'static SpanLocation,
    name: &StringId,
    recorded_deps: &mut HashSet<u64>,
    deps: &mut ThreadDepsQueue,
) {
    let location_id = thread_span_location as *const _ as u64;
    if recorded_deps.insert(location_id) {
        let target = StaticString::from(thread_span_location.target);
        if recorded_deps.insert(target.ptr as u64) {
            deps.push(target);
        }
        let module_path = StaticString::from(thread_span_location.module_path);
        if recorded_deps.insert(module_path.ptr as u64) {
            deps.push(module_path);
        }
        let file = StaticString::from(thread_span_location.file);
        if recorded_deps.insert(file.ptr as u64) {
            deps.push(file);
        }
        deps.push(SpanLocationRecord {
            id: location_id,
            target: thread_span_location.target.as_ptr(),
            module_path: thread_span_location.module_path.as_ptr(),
            file: thread_span_location.file.as_ptr(),
            line: thread_span_location.line,
            lod: thread_span_location.lod as u32,
        });
    }

    if recorded_deps.insert(name.id()) {
        deps.push(StaticString::from(name));
    }
}

impl ExtractDeps for ThreadEventQueue {
    type DepsQueue = ThreadDepsQueue;

    fn extract(&self) -> Self::DepsQueue {
        let mut deps = ThreadDepsQueue::new(1024 * 1024);
        let mut recorded_deps = HashSet::new();
        for x in self.iter() {
            match x {
                ThreadEventQueueAny::BeginThreadSpanEvent(evt) => {
                    record_scope_event_dependencies(
                        evt.thread_span_desc,
                        &mut recorded_deps,
                        &mut deps,
                    );
                }
                ThreadEventQueueAny::EndThreadSpanEvent(evt) => {
                    record_scope_event_dependencies(
                        evt.thread_span_desc,
                        &mut recorded_deps,
                        &mut deps,
                    );
                }
                ThreadEventQueueAny::BeginThreadNamedSpanEvent(evt) => {
                    record_named_scope_event_dependencies(
                        evt.thread_span_location,
                        &evt.name,
                        &mut recorded_deps,
                        &mut deps,
                    );
                }
                ThreadEventQueueAny::EndThreadNamedSpanEvent(evt) => {
                    record_named_scope_event_dependencies(
                        evt.thread_span_location,
                        &evt.name,
                        &mut recorded_deps,
                        &mut deps,
                    );
                }
                ThreadEventQueueAny::BeginAsyncSpanEvent(evt) => {
                    record_scope_event_dependencies(evt.span_desc, &mut recorded_deps, &mut deps);
                }
                ThreadEventQueueAny::EndAsyncSpanEvent(evt) => {
                    record_scope_event_dependencies(evt.span_desc, &mut recorded_deps, &mut deps);
                }
                ThreadEventQueueAny::BeginAsyncNamedSpanEvent(evt) => {
                    record_named_scope_event_dependencies(
                        evt.span_location,
                        &evt.name,
                        &mut recorded_deps,
                        &mut deps,
                    );
                }
                ThreadEventQueueAny::EndAsyncNamedSpanEvent(evt) => {
                    record_named_scope_event_dependencies(
                        evt.span_location,
                        &evt.name,
                        &mut recorded_deps,
                        &mut deps,
                    );
                }
            }
        }
        deps
    }
}

pub type ThreadBlock = EventBlock<ThreadEventQueue>;
pub type ThreadStream = EventStream<ThreadBlock>;
