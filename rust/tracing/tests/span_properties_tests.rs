use micromegas_tracing::dispatch::flush_thread_buffer;
use micromegas_tracing::prelude::*;
use micromegas_tracing::property_set::PropertySet;
use micromegas_tracing::spans::{SpanProperties, ThreadBlock, ThreadEventQueueAny};
use micromegas_tracing::test_utils::init_in_memory_tracing;
use micromegas_transit::HeterogeneousQueue;
use serial_test::serial;
use std::sync::Arc;

fn blocks(guard: &micromegas_tracing::test_utils::InMemoryTracingGuard) -> Vec<Arc<ThreadBlock>> {
    flush_thread_buffer();
    guard.sink.state.lock().unwrap().thread_blocks.clone()
}

fn sorted(mut pairs: Vec<(&str, &str)>) -> Vec<(String, String)> {
    pairs.sort();
    pairs
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The tagged and plain ends of a thread span, as (type, properties).
fn thread_ends(block: &ThreadBlock) -> Vec<(&'static str, Vec<(String, String)>)> {
    let mut out = vec![];
    for evt in block.events.iter() {
        match evt {
            ThreadEventQueueAny::EndThreadSpanEvent(_) => out.push(("plain", vec![])),
            ThreadEventQueueAny::EndThreadNamedSpanEvent(_) => out.push(("plain_named", vec![])),
            ThreadEventQueueAny::EndThreadDynNamedSpanEvent(_) => {
                out.push(("plain_dyn_named", vec![]))
            }
            ThreadEventQueueAny::TaggedEndThreadSpanEvent(e) => out.push((
                "tagged",
                sorted(block.dyn_deps.set_properties(e.properties)),
            )),
            ThreadEventQueueAny::TaggedEndThreadNamedSpanEvent(e) => out.push((
                "tagged_named",
                sorted(block.dyn_deps.set_properties(e.properties)),
            )),
            _ => {}
        }
    }
    out
}

#[test]
#[serial]
fn literals_only_resolve_as_interned_with_empty_dyn_deps() {
    let guard = init_in_memory_tracing();
    micromegas_tracing::dispatch::init_thread_stream();
    {
        let _span = span_scope!(name = "x", k = "lit");
    }
    let blocks = blocks(&guard);
    let block = &blocks[0];
    let mut found = false;
    for evt in block.events.iter() {
        if let ThreadEventQueueAny::TaggedEndThreadSpanEvent(e) = evt {
            assert!(e.properties.as_interned().is_some());
            found = true;
        }
    }
    assert!(found);
    assert_eq!(block.dyn_deps.len_bytes(), 0);
}

#[test]
#[serial]
fn runtime_values_override_literals_and_spread_entries() {
    let guard = init_in_memory_tracing();
    micromegas_tracing::dispatch::init_thread_stream();
    let id = 7;
    {
        let mut span = span_scope!(name = "x", region = "eu", request = %id);
        span.set_property("rows", 3);
        span.set_property("region", "us");
    }
    {
        let base = PropertySet::find_or_create(vec![
            micromegas_tracing::property_set::Property::new("a", "from_set"),
            micromegas_tracing::property_set::Property::new("b", "from_set"),
        ]);
        let _span = span_scope!(name = "y", a = "lit", ..base);
    }
    {
        let base =
            PropertySet::find_or_create(vec![micromegas_tracing::property_set::Property::new(
                "a", "from_set",
            )]);
        let _span = span_scope!(name = "z", ..base);
    }
    let blocks = blocks(&guard);
    let ends = thread_ends(&blocks[0]);
    assert_eq!(ends.len(), 3);
    assert_eq!(
        ends[0].1,
        owned(&[("region", "us"), ("request", "7"), ("rows", "3")])
    );
    assert_eq!(ends[1].1, owned(&[("a", "lit"), ("b", "from_set")]));
    assert_eq!(ends[2].1, owned(&[("a", "from_set")]));
}

#[test]
#[serial]
fn nothing_set_emits_the_plain_end_and_old_forms_are_unchanged() {
    let guard = init_in_memory_tracing();
    micromegas_tracing::dispatch::init_thread_stream();
    {
        let _span = span_scope!(name = "x");
    }
    {
        span_scope!("y");
    }
    let blocks = blocks(&guard);
    let ends = thread_ends(&blocks[0]);
    assert_eq!(
        ends.iter().map(|e| e.0).collect::<Vec<_>>(),
        vec!["plain", "plain"]
    );
    let begins = blocks[0]
        .events
        .iter()
        .filter(|e| matches!(e, ThreadEventQueueAny::BeginThreadSpanEvent(_)))
        .count();
    assert_eq!(begins, 2);
}

fn early_return(fail: bool) -> Result<(), ()> {
    let mut span = span_scope!(name = "early", kind = "x");
    span.set_property("step", 1);
    if fail {
        return Err(());
    }
    span.set_property("step", 2);
    Ok(())
}

#[test]
#[serial]
fn an_early_return_still_records_the_properties() {
    let guard = init_in_memory_tracing();
    micromegas_tracing::dispatch::init_thread_stream();
    let _ = early_return(true);
    let blocks = blocks(&guard);
    let ends = thread_ends(&blocks[0]);
    assert_eq!(ends[0].1, owned(&[("kind", "x"), ("step", "1")]));
}

#[test]
#[serial]
fn named_forms_emit_tagged_named_ends_with_the_name_in_dyn_deps() {
    let guard = init_in_memory_tracing();
    micromegas_tracing::dispatch::init_thread_stream();
    {
        let mut span = span_scope_named!(name = "static_name", k = "v");
        span.set_property("n", 1);
    }
    {
        let _span = span_scope_named_dyn!(name = format!("dyn_{}", 1), k = "v");
    }
    let blocks = blocks(&guard);
    let block = &blocks[0];
    let ends = thread_ends(block);
    assert_eq!(ends[0].0, "tagged_named");
    assert_eq!(ends[0].1, owned(&[("k", "v"), ("n", "1")]));
    assert_eq!(ends[1].0, "tagged_named");
    let names: Vec<&str> = block
        .events
        .iter()
        .filter_map(|e| match e {
            ThreadEventQueueAny::TaggedEndThreadNamedSpanEvent(e) => {
                Some(block.dyn_deps.str(e.name.id))
            }
            _ => None,
        })
        .collect();
    assert_eq!(names, vec!["static_name", "dyn_1"]);
}

#[test]
#[serial]
fn the_same_dynamic_set_shares_one_record() {
    let guard = init_in_memory_tracing();
    micromegas_tracing::dispatch::init_thread_stream();
    for _ in 0..2 {
        let _span = span_scope!(name = "x", id = %5);
    }
    let blocks = blocks(&guard);
    let block = &blocks[0];
    let ids: Vec<_> = block
        .events
        .iter()
        .filter_map(|e| match e {
            ThreadEventQueueAny::TaggedEndThreadSpanEvent(e) => Some(e.properties),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], ids[1]);
}

#[test]
#[serial]
fn an_async_end_hook_records_properties_with_the_span_id() {
    let guard = init_in_memory_tracing();
    micromegas_tracing::dispatch::init_thread_stream();
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(async {
        let n = instrument_named!(async { 5 }, "with", |out: &i32, span| {
            span.set_property("out", *out);
        })
        .await;
        assert_eq!(n, 5);
        instrument_named!(async { 5 }, "empty_hook", |_out: &i32, _span| {}).await;
        instrument_named!(async { 5 }, "no_hook").await;
    });
    let blocks = blocks(&guard);
    let block = &blocks[0];
    let mut begin_ids = vec![];
    let mut tagged = vec![];
    let mut plain = 0;
    for evt in block.events.iter() {
        match evt {
            ThreadEventQueueAny::BeginAsyncNamedSpanEvent(e) => begin_ids.push(e.span_id),
            ThreadEventQueueAny::TaggedEndAsyncNamedSpanEvent(e) => tagged.push((
                e.span_id,
                sorted(block.dyn_deps.set_properties(e.properties)),
            )),
            ThreadEventQueueAny::EndAsyncNamedSpanEvent(_) => plain += 1,
            _ => {}
        }
    }
    assert_eq!(tagged.len(), 1);
    assert_eq!(tagged[0].0, begin_ids[0]);
    assert_eq!(tagged[0].1, owned(&[("out", "5")]));
    assert_eq!(plain, 2);
}

#[test]
#[serial]
fn instrument_with_emits_a_tagged_unnamed_end() {
    let guard = init_in_memory_tracing();
    micromegas_tracing::dispatch::init_thread_stream();
    static_span_desc!(DESC, "fut");
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(async {
        async { 1 }
            .instrument_with(&DESC, |out, span| span.set_property("out", out))
            .await;
    });
    let blocks = blocks(&guard);
    let block = &blocks[0];
    let found = block.events.iter().any(|e| match e {
        ThreadEventQueueAny::TaggedEndAsyncSpanEvent(e) => {
            sorted(block.dyn_deps.set_properties(e.properties)) == owned(&[("out", "1")])
        }
        _ => false,
    });
    assert!(found);
}

fn dyn_pairs(props: &SpanProperties) -> Vec<(String, String)> {
    props.with_dyn(|_, dyn_properties| {
        dyn_properties
            .iter()
            .map(|p| (p.name().to_owned(), p.value().to_owned()))
            .collect()
    })
}

#[test]
fn setting_a_name_twice_keeps_the_last_value_without_growing() {
    let mut props = SpanProperties::new(PropertySet::empty());
    props.set_property("rows", 1);
    props.set_property("cache", "hit");
    let len = props.value_bytes();
    for i in 0..100 {
        props.set_property("rows", i % 10);
    }
    assert_eq!(props.value_bytes(), len);
    assert_eq!(dyn_pairs(&props), owned(&[("cache", "hit"), ("rows", "9")]));
}

#[test]
fn a_repeat_is_case_insensitive() {
    let mut props = SpanProperties::new(PropertySet::empty());
    props.set_property("Rows", 1);
    props.set_property("rows", 2);
    assert_eq!(dyn_pairs(&props), owned(&[("rows", "2")]));
}

#[test]
fn a_value_longer_than_the_inline_capacity_round_trips() {
    let long = "x".repeat(1000);
    let mut props = SpanProperties::new(PropertySet::empty());
    props.set_property("short", "a");
    props.set_property("long", &long);
    props.set_property("short", "b");
    assert_eq!(
        dyn_pairs(&props),
        owned(&[("long", long.as_str()), ("short", "b")])
    );
}
