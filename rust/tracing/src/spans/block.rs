use super::{
    BeginAsyncDynNamedSpanEvent, BeginAsyncNamedSpanEvent, BeginAsyncSpanEvent,
    BeginThreadDynNamedSpanEvent, BeginThreadNamedSpanEvent, BeginThreadSpanEvent,
    EndAsyncDynNamedSpanEvent, EndAsyncNamedSpanEvent, EndAsyncSpanEvent,
    EndThreadDynNamedSpanEvent, EndThreadNamedSpanEvent, EndThreadSpanEvent, SpanLocation,
    SpanLocationRecord, SpanMetadata, SpanRecord, TaggedEndAsyncNamedSpanEvent,
    TaggedEndAsyncSpanEvent, TaggedEndThreadNamedSpanEvent, TaggedEndThreadSpanEvent,
};
use crate::{
    dyn_deps::{DynDeps, merge_properties},
    event::{EventBlock, EventStream, ExtractDeps},
    property_set::{
        DynProperty, PropertyDepsQueue, PropertySet, PropertySetDependency, PropertySetId,
        StringDepsQueue, record_properties,
    },
    source_path::trim_source_path,
    string_id::{DynStringId, StringId},
};
use micromegas_transit::{StaticStringDependency, Utf8StaticStringDependency, prelude::*};
use rustc_hash::FxHashSet;

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
        BeginThreadDynNamedSpanEvent,
        EndThreadDynNamedSpanEvent,
        BeginAsyncDynNamedSpanEvent,
        EndAsyncDynNamedSpanEvent,
        TaggedEndThreadSpanEvent,
        TaggedEndThreadNamedSpanEvent,
        TaggedEndAsyncSpanEvent,
        TaggedEndAsyncNamedSpanEvent,
    > {}
);

declare_queue_struct!(
    struct ThreadDepsQueue<
        SpanRecord,
        SpanLocationRecord,
        Utf8StaticStringDependency,
        StaticStringDependency,
        PropertySetDependency,
    > {}
);

impl StringDepsQueue for ThreadDepsQueue {
    fn push_string(&mut self, dep: StaticStringDependency) {
        self.push(dep);
    }
}

impl PropertyDepsQueue for ThreadDepsQueue {
    fn push_set(&mut self, dep: PropertySetDependency) {
        self.push(dep);
    }
}

fn record_scope_event_dependencies(
    thread_span_desc: &'static SpanMetadata,
    recorded_deps: &mut FxHashSet<u64>,
    deps: &mut ThreadDepsQueue,
) {
    let thread_span_ptr = thread_span_desc as *const _ as u64;
    if recorded_deps.insert(thread_span_ptr) {
        let name = Utf8StaticStringDependency::from(thread_span_desc.name);
        if recorded_deps.insert(name.ptr as u64) {
            deps.push(name);
        }
        let target = Utf8StaticStringDependency::from(thread_span_desc.location.target);
        if recorded_deps.insert(target.ptr as u64) {
            deps.push(target);
        }
        let module_path = Utf8StaticStringDependency::from(thread_span_desc.location.module_path);
        if recorded_deps.insert(module_path.ptr as u64) {
            deps.push(module_path);
        }
        let file =
            Utf8StaticStringDependency::from(trim_source_path(thread_span_desc.location.file));
        let file_ptr = file.ptr;
        if recorded_deps.insert(file.ptr as u64) {
            deps.push(file);
        }
        deps.push(SpanRecord {
            id: thread_span_ptr,
            name: thread_span_desc.name.as_ptr(),
            target: thread_span_desc.location.target.as_ptr(),
            module_path: thread_span_desc.location.module_path.as_ptr(),
            file: file_ptr,
            line: thread_span_desc.location.line,
            lod: thread_span_desc.location.lod as u32,
        });
    }
}

fn record_span_location_dependencies(
    thread_span_location: &'static SpanLocation,
    recorded_deps: &mut FxHashSet<u64>,
    deps: &mut ThreadDepsQueue,
) {
    let location_id = thread_span_location as *const _ as u64;
    if recorded_deps.insert(location_id) {
        let target = Utf8StaticStringDependency::from(thread_span_location.target);
        if recorded_deps.insert(target.ptr as u64) {
            deps.push(target);
        }
        let module_path = Utf8StaticStringDependency::from(thread_span_location.module_path);
        if recorded_deps.insert(module_path.ptr as u64) {
            deps.push(module_path);
        }
        let file = Utf8StaticStringDependency::from(trim_source_path(thread_span_location.file));
        let file_ptr = file.ptr;
        if recorded_deps.insert(file.ptr as u64) {
            deps.push(file);
        }
        deps.push(SpanLocationRecord {
            id: location_id,
            target: thread_span_location.target.as_ptr(),
            module_path: thread_span_location.module_path.as_ptr(),
            file: file_ptr,
            line: thread_span_location.line,
            lod: thread_span_location.lod as u32,
        });
    }
}

fn record_named_scope_event_dependencies(
    thread_span_location: &'static SpanLocation,
    name: &StringId,
    recorded_deps: &mut FxHashSet<u64>,
    deps: &mut ThreadDepsQueue,
) {
    record_span_location_dependencies(thread_span_location, recorded_deps, deps);
    if recorded_deps.insert(name.id()) {
        deps.push(Utf8StaticStringDependency::from(name));
    }
}

fn record_last_set(
    properties: PropertySetId,
    last_set: &mut PropertySetId,
    recorded_deps: &mut FxHashSet<u64>,
    deps: &mut ThreadDepsQueue,
) {
    if properties != *last_set {
        *last_set = properties;
        if let Some(set) = properties.as_interned() {
            record_properties(set, recorded_deps, deps);
        }
    }
}

impl ExtractDeps for ThreadEventQueue {
    type DepsQueue = ThreadDepsQueue;

    fn extract(&self, dyn_deps: &DynDeps<Self::DepsQueue>) -> Self::DepsQueue {
        let mut deps = ThreadDepsQueue::new(1024 * 1024);
        let mut recorded_deps = FxHashSet::default();
        let mut last_set = PropertySetId(0);
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
                ThreadEventQueueAny::BeginThreadDynNamedSpanEvent(evt) => {
                    record_span_location_dependencies(
                        evt.thread_span_location,
                        &mut recorded_deps,
                        &mut deps,
                    );
                }
                ThreadEventQueueAny::EndThreadDynNamedSpanEvent(evt) => {
                    record_span_location_dependencies(
                        evt.thread_span_location,
                        &mut recorded_deps,
                        &mut deps,
                    );
                }
                ThreadEventQueueAny::BeginAsyncDynNamedSpanEvent(evt) => {
                    record_span_location_dependencies(
                        evt.span_location,
                        &mut recorded_deps,
                        &mut deps,
                    );
                }
                ThreadEventQueueAny::EndAsyncDynNamedSpanEvent(evt) => {
                    record_span_location_dependencies(
                        evt.span_location,
                        &mut recorded_deps,
                        &mut deps,
                    );
                }
                ThreadEventQueueAny::TaggedEndThreadSpanEvent(evt) => {
                    record_scope_event_dependencies(
                        evt.thread_span_desc,
                        &mut recorded_deps,
                        &mut deps,
                    );
                    record_last_set(evt.properties, &mut last_set, &mut recorded_deps, &mut deps);
                }
                ThreadEventQueueAny::TaggedEndThreadNamedSpanEvent(evt) => {
                    record_span_location_dependencies(
                        evt.thread_span_location,
                        &mut recorded_deps,
                        &mut deps,
                    );
                    record_last_set(evt.properties, &mut last_set, &mut recorded_deps, &mut deps);
                }
                ThreadEventQueueAny::TaggedEndAsyncSpanEvent(evt) => {
                    record_scope_event_dependencies(evt.span_desc, &mut recorded_deps, &mut deps);
                    record_last_set(evt.properties, &mut last_set, &mut recorded_deps, &mut deps);
                }
                ThreadEventQueueAny::TaggedEndAsyncNamedSpanEvent(evt) => {
                    record_span_location_dependencies(
                        evt.span_location,
                        &mut recorded_deps,
                        &mut deps,
                    );
                    record_last_set(evt.properties, &mut last_set, &mut recorded_deps, &mut deps);
                }
            }
        }
        // The parser needs the `'static` names and values of dynamic sets before the queue that
        // references them.
        dyn_deps.record_static_refs(&mut recorded_deps, &mut deps);
        deps.extend_from(dyn_deps.queue());
        deps
    }
}

pub type ThreadBlock = EventBlock<ThreadEventQueue>;
pub type ThreadStream = EventStream<ThreadBlock>;

impl ThreadStream {
    /// Writes `name` to the current block's dynamic queue, then pushes the event built from its id.
    pub fn push_dyn_named<T>(&mut self, name: &str, make: impl FnOnce(DynStringId) -> T)
    where
        T: InProcSerialize + ThreadEventQueueTypeIndex,
    {
        let (events, dyn_deps) = self.get_block_mut_parts();
        let id = dyn_deps.intern_str(name);
        events.push(make(DynStringId { id }));
    }

    /// Pushes the event built from the id of `set` combined with `dyn_properties`.
    pub fn push_tagged<T>(
        &mut self,
        set: &'static PropertySet,
        dyn_properties: &[DynProperty<'_>],
        make: impl FnOnce(PropertySetId) -> T,
    ) where
        T: InProcSerialize + ThreadEventQueueTypeIndex,
    {
        let (events, dyn_deps) = self.get_block_mut_parts();
        let properties = tagged_set_id(dyn_deps, set, dyn_properties);
        events.push(make(properties));
    }

    /// Like `push_tagged`, for an event that also names a span with a block-scoped string.
    pub fn push_tagged_named<T>(
        &mut self,
        name: &str,
        set: &'static PropertySet,
        dyn_properties: &[DynProperty<'_>],
        make: impl FnOnce(DynStringId, PropertySetId) -> T,
    ) where
        T: InProcSerialize + ThreadEventQueueTypeIndex,
    {
        let (events, dyn_deps) = self.get_block_mut_parts();
        let id = dyn_deps.intern_str(name);
        let properties = tagged_set_id(dyn_deps, set, dyn_properties);
        events.push(make(DynStringId { id }, properties));
    }
}

fn tagged_set_id(
    dyn_deps: &mut DynDeps<ThreadDepsQueue>,
    set: &'static PropertySet,
    dyn_properties: &[DynProperty<'_>],
) -> PropertySetId {
    if dyn_properties.is_empty() {
        return PropertySetId::interned(set);
    }
    dyn_deps.intern_set(&merge_properties(set, dyn_properties))
}
