use micromegas_tracing::dispatch::flush_thread_buffer;
use micromegas_tracing::dyn_deps::{DYN_ID_TAG, DynDeps};
use micromegas_tracing::prelude::*;
use micromegas_tracing::spans::{ThreadBlock, ThreadDepsQueue, ThreadEventQueueAny};
use micromegas_tracing::test_utils::init_in_memory_tracing;
use micromegas_transit::HeterogeneousQueue;
use serial_test::serial;
use std::sync::Arc;

fn record_size(name: &str) -> usize {
    let mut deps = DynDeps::<ThreadDepsQueue>::new();
    deps.intern_str(name);
    deps.len_bytes()
}

fn thread_blocks(
    guard: &micromegas_tracing::test_utils::InMemoryTracingGuard,
) -> Vec<Arc<ThreadBlock>> {
    guard.sink.state.lock().unwrap().thread_blocks.clone()
}

#[test]
#[serial]
fn same_name_shares_one_record() {
    let guard = init_in_memory_tracing();
    micromegas_tracing::dispatch::init_thread_stream();
    let name = String::from("runtime_name");
    {
        span_scope_named_dyn!(&name);
    }
    {
        span_scope_named_dyn!(format!("runtime_{}", "name"));
    }
    flush_thread_buffer();

    let blocks = thread_blocks(&guard);
    assert_eq!(blocks.len(), 1);
    let block = &blocks[0];
    assert_eq!(block.dyn_deps.len_bytes(), record_size("runtime_name"));
    let ids: Vec<u64> = block
        .events
        .iter()
        .map(|evt| match evt {
            ThreadEventQueueAny::BeginThreadDynNamedSpanEvent(e) => e.name.id,
            ThreadEventQueueAny::EndThreadDynNamedSpanEvent(e) => e.name.id,
            _ => panic!("unexpected event type"),
        })
        .collect();
    assert_eq!(ids.len(), 4);
    assert!(ids.iter().all(|id| *id == ids[0]));
    assert_ne!(ids[0] & DYN_ID_TAG, 0);
}

#[test]
#[serial]
fn distinct_names_have_distinct_ids() {
    let guard = init_in_memory_tracing();
    micromegas_tracing::dispatch::init_thread_stream();
    {
        span_scope_named_dyn!(String::from("first"));
    }
    {
        span_scope_named_dyn!(String::from("second"));
    }
    flush_thread_buffer();

    let blocks = thread_blocks(&guard);
    let ids: Vec<u64> = blocks[0]
        .events
        .iter()
        .map(|evt| match evt {
            ThreadEventQueueAny::BeginThreadDynNamedSpanEvent(e) => e.name.id,
            ThreadEventQueueAny::EndThreadDynNamedSpanEvent(e) => e.name.id,
            _ => panic!("unexpected event type"),
        })
        .collect();
    assert_eq!(ids[0], ids[1]);
    assert_eq!(ids[2], ids[3]);
    assert_ne!(ids[0], ids[2]);
    assert_eq!(
        blocks[0].dyn_deps.len_bytes(),
        record_size("first") + record_size("second")
    );
}

#[test]
#[serial]
fn static_named_span_leaves_dyn_deps_empty() {
    let guard = init_in_memory_tracing();
    micromegas_tracing::dispatch::init_thread_stream();
    {
        span_scope_named!("static_name");
    }
    flush_thread_buffer();

    let blocks = thread_blocks(&guard);
    let block = &blocks[0];
    assert_eq!(block.dyn_deps.len_bytes(), 0);
    assert_eq!(block.events.nb_objects(), 2);
    for evt in block.events.iter() {
        assert!(matches!(
            evt,
            ThreadEventQueueAny::BeginThreadNamedSpanEvent(_)
                | ThreadEventQueueAny::EndThreadNamedSpanEvent(_)
        ));
    }
}

#[test]
#[serial]
fn end_in_a_new_block_writes_the_name_again() {
    let guard = init_in_memory_tracing();
    micromegas_tracing::dispatch::init_thread_stream();
    {
        span_scope_named_dyn!(String::from("straddling"));
        flush_thread_buffer();
    }
    flush_thread_buffer();

    let blocks = thread_blocks(&guard);
    assert_eq!(blocks.len(), 2);
    let expected_id = DYN_ID_TAG; // first record of a block sits at offset 0
    for (block, is_begin) in blocks.iter().zip([true, false]) {
        assert_eq!(block.dyn_deps.len_bytes(), record_size("straddling"));
        let evt = block.events.iter().next().unwrap();
        let id = match (evt, is_begin) {
            (ThreadEventQueueAny::BeginThreadDynNamedSpanEvent(e), true) => e.name.id,
            (ThreadEventQueueAny::EndThreadDynNamedSpanEvent(e), false) => e.name.id,
            _ => panic!("unexpected event type"),
        };
        assert_eq!(id, expected_id);
    }
}

#[test]
#[serial]
fn instrument_named_dyn_with_owned_name() {
    let guard = init_in_memory_tracing();
    micromegas_tracing::dispatch::init_thread_stream();
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(async {
        let fut = instrument_named_dyn!(async { 42 }, format!("async_{}", 1));
        assert_eq!(fut.await, 42);
    });
    flush_thread_buffer();

    let blocks = thread_blocks(&guard);
    let block = &blocks[0];
    assert_eq!(block.dyn_deps.len_bytes(), record_size("async_1"));
    let mut kinds = vec![];
    for evt in block.events.iter() {
        match evt {
            ThreadEventQueueAny::BeginAsyncDynNamedSpanEvent(e) => {
                assert_ne!(e.name.id & DYN_ID_TAG, 0);
                kinds.push("begin");
            }
            ThreadEventQueueAny::EndAsyncDynNamedSpanEvent(e) => {
                assert_ne!(e.name.id & DYN_ID_TAG, 0);
                kinds.push("end");
            }
            _ => {}
        }
    }
    assert_eq!(kinds, ["begin", "end"]);
}
