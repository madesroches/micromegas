use datafusion::arrow::array::{Array, AsArray, TimestampNanosecondArray};
use datafusion::arrow::datatypes::Int32Type;
use micromegas_analytics::arrow_properties::serialize_property_set_to_jsonb;
use micromegas_analytics::call_tree::CallTreeBuilder;
use micromegas_analytics::dfext::typed_column::typed_column_by_name;
use micromegas_analytics::lakehouse::thread_spans_view::ensure_begin_non_decreasing;
use micromegas_analytics::properties::property_set::PropertySet;
use micromegas_analytics::scope::BorrowedScopeDesc;
use micromegas_analytics::span_table::SpanRecordBuilder;
use micromegas_analytics::thread_block_processor::ThreadBlockProcessor;
use micromegas_analytics::time::ConvertTicks;
use micromegas_transit::value::{Object, Value};

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
    let rows = rows_object("7");
    builder
        .on_end_thread_scope("block2", 2, scope("A"), 300, PropertySet::new(&rows))
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
    assert_eq!(
        tree.property_sets.values()[span_a.properties as usize],
        jsonb(&PropertySet::new(&rows)),
        "the end properties arrive with the end event, in the second block"
    );
}

fn rows_object(value: &'static str) -> Object<'static> {
    Object {
        type_name: "PropertySet",
        members: Box::leak(Box::new([("rows", Value::String(value))])),
    }
}

fn jsonb(properties: &PropertySet<'_>) -> Vec<u8> {
    serialize_property_set_to_jsonb(properties).expect("jsonb")
}

/// The JSONB of every row's `properties`, in row order.
fn row_properties(batch: &datafusion::arrow::record_batch::RecordBatch) -> Vec<Vec<u8>> {
    let dict = batch
        .column_by_name("properties")
        .expect("properties column")
        .as_dictionary::<Int32Type>();
    let values = dict.values().as_binary::<i32>();
    (0..dict.len())
        .map(|i| values.value(dict.keys().value(i) as usize).to_vec())
        .collect()
}

fn build_batch(
    builder: &mut SpanRecordBuilder,
    trees: Vec<micromegas_analytics::call_tree::CallTree>,
) -> usize {
    for tree in &trees {
        builder.append_call_tree(tree).expect("append_call_tree");
    }
    trees.len()
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
    let result = builder.on_end_thread_scope("block1", 3, scope("A"), 300, PropertySet::empty());
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

/// Pins one of the two properties `max_sort_key_time` (the segment-boundary overlap fix,
/// `tasks/completed/thread_spans_segment_boundary_overlap_plan.md`) rests on: an event outside the chain's
/// `[begin_range_ns, end_range_ns]` is **dropped, not clamped in** (`call_tree.rs`'s
/// `on_begin_thread_scope`/`on_end_thread_scope`), so no row's `begin` can ever escape its
/// partition's declared range.
#[test]
fn test_out_of_range_events_are_dropped_not_clamped() {
    let convert = make_convert_ticks();
    // Chain range is [1000, 2000] ns.
    let mut builder = CallTreeBuilder::new(1000, 2000, None, convert, "test_thread".into());

    // A span entirely before the range: both begin and end are before begin_range_ns.
    builder
        .on_begin_thread_scope("block1", 1, scope("before"), 100)
        .expect("begin before-range");
    builder
        .on_end_thread_scope("block1", 2, scope("before"), 200, PropertySet::empty())
        .expect("end before-range");

    // A span inside the range.
    builder
        .on_begin_thread_scope("block1", 3, scope("inside"), 1500)
        .expect("begin inside-range");
    builder
        .on_end_thread_scope("block1", 4, scope("inside"), 1800, PropertySet::empty())
        .expect("end inside-range");

    // A span entirely after the range: both begin and end are after end_range_ns.
    builder
        .on_begin_thread_scope("block1", 5, scope("after"), 2500)
        .expect("begin after-range");
    builder
        .on_end_thread_scope("block1", 6, scope("after"), 2600, PropertySet::empty())
        .expect("end after-range");

    let tree = builder.finish();
    let root = tree.call_tree_root.expect("call tree root");
    assert_eq!(
        root.children.len(),
        1,
        "only the in-range span should produce a node -- the out-of-range ones must be dropped \
         entirely, not clamped into [begin_range_ns, end_range_ns]; got {:?}",
        root.children
    );
    let only_child = &root.children[0];
    assert_eq!(
        only_child.begin, 1500,
        "the surviving span's begin must be its real timestamp, not a clamped range bound"
    );
    assert_eq!(
        only_child.end, 1800,
        "the surviving span's end must be its real timestamp, not a clamped range bound"
    );
}

/// Pins the other property `max_sort_key_time` rests on: preorder rows are non-decreasing on
/// `begin`, so the *last* row in a finished batch is the partition-global max -- the single
/// assumption behind "read the last row's `begin`" (`thread_spans_view.rs::write_partition`).
#[test]
fn test_preorder_rows_are_non_decreasing_and_last_row_is_max() {
    let convert = make_convert_ticks();
    let mut builder = CallTreeBuilder::new(0, 1_000_000_000, None, convert, "test_thread".into());

    // Span A, with two nested children, followed by sibling span B that starts only after A (and
    // both its children) have ended -- the ordinary single-stack shape a real thread produces.
    builder
        .on_begin_thread_scope("block1", 1, scope("A"), 100)
        .expect("begin A");
    builder
        .on_begin_thread_scope("block1", 2, scope("A1"), 150)
        .expect("begin A1");
    builder
        .on_end_thread_scope("block1", 3, scope("A1"), 200, PropertySet::empty())
        .expect("end A1");
    builder
        .on_begin_thread_scope("block1", 4, scope("A2"), 250)
        .expect("begin A2");
    builder
        .on_end_thread_scope("block1", 5, scope("A2"), 300, PropertySet::empty())
        .expect("end A2");
    builder
        .on_end_thread_scope("block1", 6, scope("A"), 500, PropertySet::empty())
        .expect("end A");
    builder
        .on_begin_thread_scope("block1", 7, scope("B"), 600)
        .expect("begin B");
    builder
        .on_end_thread_scope("block1", 8, scope("B"), 700, PropertySet::empty())
        .expect("end B");

    let tree = builder.finish();
    let mut record_builder = SpanRecordBuilder::with_capacity(8);
    record_builder
        .append_call_tree(&tree)
        .expect("append_call_tree");
    let batch = record_builder.finish().expect("finish");

    // Non-decreasing on begin is exactly the invariant thread_spans_view relies on, checked here
    // the same way write_partition checks it.
    ensure_begin_non_decreasing("test-stream", &batch).expect("begin must be non-decreasing");

    let begins: &TimestampNanosecondArray =
        typed_column_by_name(&batch, "begin").expect("begin column");
    assert!(begins.len() > 1, "expected more than one row");
    let max_begin = (0..begins.len()).map(|i| begins.value(i)).max().unwrap();
    let last_begin = begins.value(begins.len() - 1);
    assert_eq!(
        last_begin, max_begin,
        "the last preorder row's begin must be the batch's max begin"
    );
}

#[test]
fn test_end_properties_reach_the_thread_spans_rows() {
    let convert = make_convert_ticks();
    let mut builder = CallTreeBuilder::new(0, 1_000_000_000, None, convert, "test_thread".into());
    let rows = rows_object("7");
    builder
        .on_begin_thread_scope("b", 1, scope("A"), 100)
        .unwrap();
    builder
        .on_begin_thread_scope("b", 2, scope("B"), 150)
        .unwrap();
    builder
        .on_end_thread_scope("b", 3, scope("B"), 200, PropertySet::new(&rows))
        .unwrap();
    builder
        .on_end_thread_scope("b", 4, scope("A"), 300, PropertySet::empty())
        .unwrap();
    // left open at the end of the range
    builder
        .on_begin_thread_scope("b", 5, scope("open"), 400)
        .unwrap();
    let tree = builder.finish();

    let mut record_builder = SpanRecordBuilder::with_capacity(4);
    build_batch(&mut record_builder, vec![tree]);
    let batch = record_builder.finish().unwrap();
    assert_eq!(batch.schema().fields().last().unwrap().name(), "properties");
    let empty = jsonb(&PropertySet::empty());
    let with_rows = jsonb(&PropertySet::new(&rows));
    // preorder: root, A, B, open
    assert_eq!(
        row_properties(&batch),
        vec![empty.clone(), empty.clone(), with_rows, empty]
    );
}

#[test]
fn test_end_properties_survive_a_begin_before_the_range() {
    let convert = make_convert_ticks();
    let mut builder = CallTreeBuilder::new(1000, 2000, None, convert, "test_thread".into());
    let outer = rows_object("outer");
    let inner = rows_object("inner");
    // both began before the range; only the ends are seen. The inner end hits the empty stack,
    // the outer end hits root adoption.
    builder
        .on_end_thread_scope("b", 1, scope("inner"), 1100, PropertySet::new(&inner))
        .unwrap();
    builder
        .on_end_thread_scope("b", 2, scope("outer"), 1200, PropertySet::new(&outer))
        .unwrap();
    let tree = builder.finish();
    let root = tree.call_tree_root.as_ref().unwrap();
    assert_eq!(root.children.len(), 1);
    let outer_node = &root.children[0];
    assert_eq!(outer_node.children.len(), 1);
    let inner_node = &outer_node.children[0];
    assert_eq!(
        tree.property_sets.values()[outer_node.properties as usize],
        jsonb(&PropertySet::new(&outer))
    );
    assert_eq!(
        tree.property_sets.values()[inner_node.properties as usize],
        jsonb(&PropertySet::new(&inner))
    );
}

#[test]
fn test_trees_sharing_a_set_share_one_dictionary_value() {
    let rows = rows_object("7");
    let make_tree = || {
        let mut builder = CallTreeBuilder::new(
            0,
            1_000_000_000,
            None,
            make_convert_ticks(),
            "test_thread".into(),
        );
        builder
            .on_begin_thread_scope("b", 1, scope("A"), 100)
            .unwrap();
        builder
            .on_end_thread_scope("b", 2, scope("A"), 200, PropertySet::new(&rows))
            .unwrap();
        builder.finish()
    };
    let mut record_builder = SpanRecordBuilder::with_capacity(4);
    build_batch(&mut record_builder, vec![make_tree(), make_tree()]);
    let batch = record_builder.finish().unwrap();
    let dict = batch
        .column_by_name("properties")
        .unwrap()
        .as_dictionary::<Int32Type>();
    // the empty set and one shared set
    assert_eq!(dict.values().len(), 2);
    assert_eq!(batch.num_rows(), 4);
}
