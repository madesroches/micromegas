use micromegas_analytics::call_tree::CallTreeBuilder;
use micromegas_analytics::scope::ScopeDesc;
use micromegas_analytics::thread_block_processor::ThreadBlockProcessor;
use micromegas_analytics::time::ConvertTicks;
use std::sync::Arc;

fn make_convert_ticks() -> ConvertTicks {
    ConvertTicks::from_meta_data(0, 0, 1_000_000_000).expect("ConvertTicks::from_meta_data")
}

fn scope(name: &str) -> ScopeDesc {
    ScopeDesc::new(
        Arc::new(name.to_string()),
        Arc::new(String::new()),
        Arc::new(String::new()),
        0,
    )
}

#[test]
fn test_crossing_spans_returns_err() {
    let convert = make_convert_ticks();
    let mut builder = CallTreeBuilder::new(0, 1_000_000_000, None, convert, "test_thread".into());

    // BeginA at ts=100
    builder
        .on_begin_thread_scope("block1", 1, scope("A"), 100)
        .expect("begin A");
    // BeginB at ts=200
    builder
        .on_begin_thread_scope("block1", 2, scope("B"), 200)
        .expect("begin B");
    // EndA at ts=300 — mismatches B on top of stack
    let result = builder.on_end_thread_scope("block1", 3, scope("A"), 300);
    assert!(result.is_err(), "expected Err for crossing spans");
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("block1"),
        "error should mention block id; got: {msg}"
    );
    assert!(
        msg.contains('A'),
        "error should mention closing scope 'A'; got: {msg}"
    );
    assert!(
        msg.contains('B'),
        "error should mention open scope 'B'; got: {msg}"
    );
}
