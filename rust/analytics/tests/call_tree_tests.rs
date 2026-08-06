use micromegas_analytics::call_tree::CallTreeBuilder;
use micromegas_analytics::scope::BorrowedScopeDesc;
use micromegas_analytics::thread_block_processor::ThreadBlockProcessor;
use micromegas_analytics::time::ConvertTicks;

/// A span opened in one block and closed in the next must come back as a single node spanning both,
/// which is what makes `thread_spans_view` feed a whole chain of contiguous blocks to one
/// `CallTreeBuilder` (see `jit_partitions::group_contiguous_block_chains`). Blocks are fed in the
/// overlapping-seam shape `micromegas_tracing` produces: block2 begins at ts=150, before block1's
/// last event at ts=200.
#[test]
fn test_span_crossing_a_block_boundary_is_one_node() {
    let convert = make_convert_ticks();
    let mut builder = CallTreeBuilder::new(0, 1_000_000_000, None, convert, "test_thread".into());

    // BeginA in block1 at ts=100, still open when the block ends.
    builder
        .on_begin_thread_scope("block1", 1, scope("A"), 100)
        .expect("begin A in block1");
    // EndA arrives in block2 at ts=300.
    builder
        .on_end_thread_scope("block2", 2, scope("A"), 300)
        .expect("end A in block2");

    let tree = builder.finish();
    let root = tree.call_tree_root.expect("call tree root");
    assert_eq!(
        root.children.len(),
        1,
        "expected exactly one top-level span, got {:?}",
        root.children
    );
    let span_a = &root.children[0];
    assert_eq!(span_a.begin, 100, "span should begin at its block1 begin");
    assert_eq!(span_a.end, 300, "span should end at its block2 end");
    assert!(
        span_a.children.is_empty(),
        "span A should have no children; got {:?}",
        span_a.children
    );
}

fn make_convert_ticks() -> ConvertTicks {
    ConvertTicks::from_meta_data(0, 0, 1_000_000_000).expect("ConvertTicks::from_meta_data")
}

fn scope(name: &str) -> BorrowedScopeDesc<'_> {
    BorrowedScopeDesc::new(name, "", "", 0)
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
