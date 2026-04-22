use micromegas_analytics::net_block_processing::NetBlockProcessor;
use micromegas_analytics::net_span_tree::{NetSpanTreeBuilder, ROOT_PARENT_SPAN_ID};
use micromegas_analytics::net_spans_table::NetSpanRecordBuilder;
use micromegas_analytics::time::ConvertTicks;
use std::sync::Arc;

fn make_builder_ctx() -> (NetSpanRecordBuilder, Arc<String>, Arc<String>, ConvertTicks) {
    // 1 tick == 1 ns, process starts at t=0. Identity conversion keeps assertions
    // easy to read (event times come out equal to the raw ticks we pass in).
    let convert_ticks = ConvertTicks::from_meta_data(0, 0, 1_000_000_000).unwrap();
    (
        NetSpanRecordBuilder::with_capacity(16),
        Arc::new(String::from("proc-1")),
        Arc::new(String::from("stream-1")),
        convert_ticks,
    )
}

fn s(value: &str) -> Arc<String> {
    Arc::new(String::from(value))
}

fn collect_rows(builder: NetSpanRecordBuilder) -> Vec<NetRow> {
    use datafusion::arrow::array::{
        BooleanArray, DictionaryArray, Int64Array, StringArray, TimestampNanosecondArray,
        UInt32Array,
    };
    use datafusion::arrow::datatypes::Int16Type;

    let batch = builder.finish().expect("finish record builder");
    let num = batch.num_rows();
    let process_ids = batch
        .column_by_name("process_id")
        .unwrap()
        .as_any()
        .downcast_ref::<DictionaryArray<Int16Type>>()
        .unwrap();
    let stream_ids = batch
        .column_by_name("stream_id")
        .unwrap()
        .as_any()
        .downcast_ref::<DictionaryArray<Int16Type>>()
        .unwrap();
    let span_ids = batch
        .column_by_name("span_id")
        .unwrap()
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap();
    let parents = batch
        .column_by_name("parent_span_id")
        .unwrap()
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap();
    let depths = batch
        .column_by_name("depth")
        .unwrap()
        .as_any()
        .downcast_ref::<UInt32Array>()
        .unwrap();
    let kinds = batch
        .column_by_name("kind")
        .unwrap()
        .as_any()
        .downcast_ref::<DictionaryArray<Int16Type>>()
        .unwrap();
    let names = batch
        .column_by_name("name")
        .unwrap()
        .as_any()
        .downcast_ref::<DictionaryArray<Int16Type>>()
        .unwrap();
    let connection_names = batch
        .column_by_name("connection_name")
        .unwrap()
        .as_any()
        .downcast_ref::<DictionaryArray<Int16Type>>()
        .unwrap();
    let is_outgoings = batch
        .column_by_name("is_outgoing")
        .unwrap()
        .as_any()
        .downcast_ref::<BooleanArray>()
        .unwrap();
    let begin_bits = batch
        .column_by_name("begin_bits")
        .unwrap()
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap();
    let end_bits = batch
        .column_by_name("end_bits")
        .unwrap()
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap();
    let bit_sizes = batch
        .column_by_name("bit_size")
        .unwrap()
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap();
    let begin_times = batch
        .column_by_name("begin_time")
        .unwrap()
        .as_any()
        .downcast_ref::<TimestampNanosecondArray>()
        .unwrap();
    let end_times = batch
        .column_by_name("end_time")
        .unwrap()
        .as_any()
        .downcast_ref::<TimestampNanosecondArray>()
        .unwrap();

    let fetch_dict = |arr: &DictionaryArray<Int16Type>, idx: usize| -> String {
        let values = arr.values().as_any().downcast_ref::<StringArray>().unwrap();
        let key = arr.keys().value(idx);
        values.value(key as usize).to_string()
    };

    (0..num)
        .map(|i| NetRow {
            process_id: fetch_dict(process_ids, i),
            stream_id: fetch_dict(stream_ids, i),
            span_id: span_ids.value(i),
            parent_span_id: parents.value(i),
            depth: depths.value(i),
            kind: fetch_dict(kinds, i),
            name: fetch_dict(names, i),
            connection_name: fetch_dict(connection_names, i),
            is_outgoing: is_outgoings.value(i),
            begin_bits: begin_bits.value(i),
            end_bits: end_bits.value(i),
            bit_size: bit_sizes.value(i),
            begin_time: begin_times.value(i),
            end_time: end_times.value(i),
        })
        .collect()
}

#[derive(Debug, Clone)]
struct NetRow {
    #[allow(dead_code)]
    process_id: String,
    #[allow(dead_code)]
    stream_id: String,
    span_id: i64,
    parent_span_id: i64,
    depth: u32,
    kind: String,
    name: String,
    connection_name: String,
    is_outgoing: bool,
    begin_bits: i64,
    end_bits: i64,
    bit_size: i64,
    begin_time: i64,
    end_time: i64,
}

fn row_by_kind_name<'a>(rows: &'a [NetRow], kind: &str, name: &str) -> &'a NetRow {
    rows.iter()
        .find(|r| r.kind == kind && r.name == name)
        .unwrap_or_else(|| panic!("no row with kind={kind} name={name}"))
}

#[test]
fn classic_hierarchy_builds_flat_tree() {
    let (mut rb, pid, sid, conv) = make_builder_ctx();
    {
        let mut b = NetSpanTreeBuilder::new(&mut rb, pid, sid, conv);
        b.on_connection_begin(1, 10, s("127.0.0.1:7777"), false)
            .unwrap();
        b.on_object_begin(2, 11, s("ObjA")).unwrap();
        b.on_property(3, 12, s("PropA1"), 8).unwrap();
        b.on_property(4, 13, s("PropA2"), 16).unwrap();
        b.on_object_end(5, 14, 32).unwrap(); // framing gap of 8 bits
        b.on_object_begin(6, 15, s("ObjB")).unwrap();
        b.on_property(7, 16, s("PropB1"), 4).unwrap();
        b.on_object_end(8, 17, 6).unwrap();
        b.on_connection_end(9, 18, 40).unwrap();
        b.finish();
    }
    let rows = collect_rows(rb);
    assert_eq!(rows.len(), 6, "expected 6 rows, got {:?}", rows);

    // Properties under ObjA.
    let p1 = row_by_kind_name(&rows, "property", "PropA1");
    let p2 = row_by_kind_name(&rows, "property", "PropA2");
    assert_eq!(p1.parent_span_id, 2);
    assert_eq!(p1.begin_bits, 0);
    assert_eq!(p1.end_bits, 8);
    assert_eq!(p1.bit_size, 8);
    assert_eq!(p1.begin_time, p1.end_time);
    assert_eq!(p2.parent_span_id, 2);
    assert_eq!(p2.begin_bits, 8);
    assert_eq!(p2.end_bits, 24);
    assert_eq!(p2.connection_name, "127.0.0.1:7777");
    assert!(!p2.is_outgoing);

    let obj_a = row_by_kind_name(&rows, "object", "ObjA");
    assert_eq!(obj_a.span_id, 2);
    assert_eq!(obj_a.parent_span_id, 1);
    assert_eq!(obj_a.depth, 1);
    assert_eq!(obj_a.begin_bits, 0);
    assert_eq!(obj_a.end_bits, 32);
    assert_eq!(obj_a.bit_size, 32);

    let obj_b = row_by_kind_name(&rows, "object", "ObjB");
    assert_eq!(obj_b.parent_span_id, 1);
    assert_eq!(obj_b.depth, 1);
    assert_eq!(obj_b.begin_bits, 32);
    assert_eq!(obj_b.end_bits, 38);
    assert_eq!(obj_b.bit_size, 6);

    let conn = row_by_kind_name(&rows, "connection", "127.0.0.1:7777");
    assert_eq!(conn.parent_span_id, ROOT_PARENT_SPAN_ID);
    assert_eq!(conn.depth, 0);
    assert_eq!(conn.begin_bits, 0);
    assert_eq!(conn.end_bits, 40);
    assert_eq!(conn.bit_size, 40);

    // Framing gap: parent bit_size (32) > sum of child bits (8+16=24).
    let prop_sum: i64 = rows
        .iter()
        .filter(|r| r.parent_span_id == obj_a.span_id)
        .map(|r| r.bit_size)
        .sum();
    assert!(obj_a.bit_size > prop_sum);
}

#[test]
fn iris_nested_object_hierarchy() {
    let (mut rb, pid, sid, conv) = make_builder_ctx();
    {
        let mut b = NetSpanTreeBuilder::new(&mut rb, pid, sid, conv);
        b.on_connection_begin(1, 100, s("conn"), true).unwrap();
        b.on_object_begin(2, 101, s("Outer")).unwrap();
        b.on_object_begin(3, 102, s("Inner")).unwrap();
        b.on_property(4, 103, s("p"), 12).unwrap();
        b.on_object_end(5, 104, 12).unwrap();
        b.on_object_end(6, 105, 14).unwrap();
        b.on_connection_end(7, 106, 16).unwrap();
        b.finish();
    }
    let rows = collect_rows(rb);
    let inner = row_by_kind_name(&rows, "object", "Inner");
    let outer = row_by_kind_name(&rows, "object", "Outer");
    assert_eq!(inner.depth, 2);
    assert_eq!(inner.parent_span_id, outer.span_id);
    assert_eq!(outer.depth, 1);
    let prop = row_by_kind_name(&rows, "property", "p");
    assert_eq!(prop.depth, 3);
    assert_eq!(prop.parent_span_id, inner.span_id);
    assert!(prop.is_outgoing);
}

#[test]
fn rpc_under_connection() {
    let (mut rb, pid, sid, conv) = make_builder_ctx();
    {
        let mut b = NetSpanTreeBuilder::new(&mut rb, pid, sid, conv);
        b.on_connection_begin(1, 1, s("conn"), false).unwrap();
        b.on_rpc_begin(2, 2, s("ClientSay")).unwrap();
        b.on_rpc_end(3, 3, 64).unwrap();
        b.on_connection_end(4, 4, 96).unwrap();
        b.finish();
    }
    let rows = collect_rows(rb);
    let rpc = row_by_kind_name(&rows, "rpc", "ClientSay");
    assert_eq!(rpc.depth, 1);
    assert_eq!(rpc.parent_span_id, 1);
    assert_eq!(rpc.begin_bits, 0);
    assert_eq!(rpc.end_bits, 64);
}

#[test]
fn cross_block_stitching_single_connection_row() {
    let (mut rb, pid, sid, conv) = make_builder_ctx();
    {
        // Drive events continuously, simulating a Begin in block N and an End in block N+1
        // by not resetting builder state between the two halves.
        let mut b = NetSpanTreeBuilder::new(&mut rb, pid, sid, conv);
        b.on_connection_begin(42, 1000, s("stitched"), false)
            .unwrap();
        // "block boundary" — no state reset
        b.on_property(43, 1500, s("p"), 8).unwrap();
        b.on_connection_end(44, 2000, 24).unwrap();
        b.finish();
    }
    let rows = collect_rows(rb);
    // One connection row and one property row = 2 rows.
    assert_eq!(rows.len(), 2);
    let conn = row_by_kind_name(&rows, "connection", "stitched");
    assert_eq!(
        conn.span_id, 42,
        "span_id must equal the Begin event's event_id, proving global uniqueness across blocks"
    );
    assert_eq!(conn.begin_time, 1000);
    assert_eq!(conn.end_time, 2000);
    assert_eq!(conn.bit_size, 24);
}

#[test]
fn unclosed_connection_is_dropped() {
    let (mut rb, pid, sid, conv) = make_builder_ctx();
    {
        let mut b = NetSpanTreeBuilder::new(&mut rb, pid, sid, conv);
        b.on_connection_begin(7, 10, s("orphan"), false).unwrap();
        // No matching end before finish().
        b.finish();
    }
    let rows = collect_rows(rb);
    assert!(
        rows.is_empty(),
        "unclosed spans should be dropped, got {:?}",
        rows
    );
}

#[test]
fn mismatched_end_does_not_pop_stack() {
    // If a NetConnectionEndEvent arrives while an Object is on top (malformed
    // stream), popping the Object and emitting it with the Connection's bit_size
    // would corrupt attribution and strand the real Connection. close_span must
    // skip the mismatched End, leaving the Object free to be closed normally.
    let (mut rb, pid, sid, conv) = make_builder_ctx();
    {
        let mut b = NetSpanTreeBuilder::new(&mut rb, pid, sid, conv);
        b.on_connection_begin(1, 10, s("conn"), false).unwrap();
        b.on_object_begin(2, 11, s("Obj")).unwrap();
        // Mismatched: ConnectionEnd while Object is on top. Must be skipped.
        b.on_connection_end(99, 12, 9999).unwrap();
        // Now close normally; outputs should be correct.
        b.on_object_end(3, 13, 16).unwrap();
        b.on_connection_end(4, 14, 24).unwrap();
        b.finish();
    }
    let rows = collect_rows(rb);
    let obj = row_by_kind_name(&rows, "object", "Obj");
    assert_eq!(
        obj.bit_size, 16,
        "Object must close with its own End's bit_size, not the mismatched 9999"
    );
    let conn = row_by_kind_name(&rows, "connection", "conn");
    assert_eq!(conn.bit_size, 24);
    assert_eq!(rows.len(), 2, "expected exactly two rows, got {:?}", rows);
}

#[test]
fn connection_at_event_id_zero_does_not_self_reference() {
    // The first block of a stream has object_offset = 0, so the very first event
    // legitimately has event_id = 0. The root sentinel must not collide with it.
    let (mut rb, pid, sid, conv) = make_builder_ctx();
    {
        let mut b = NetSpanTreeBuilder::new(&mut rb, pid, sid, conv);
        b.on_connection_begin(0, 10, s("conn"), false).unwrap();
        b.on_property(1, 11, s("p"), 8).unwrap();
        b.on_connection_end(2, 12, 8).unwrap();
        b.finish();
    }
    let rows = collect_rows(rb);
    let conn = row_by_kind_name(&rows, "connection", "conn");
    assert_eq!(conn.span_id, 0);
    assert_eq!(
        conn.parent_span_id, ROOT_PARENT_SPAN_ID,
        "root Connection with event_id 0 must not self-reference; parent must be the sentinel"
    );
    let prop = row_by_kind_name(&rows, "property", "p");
    assert_eq!(
        prop.parent_span_id, 0,
        "the property's parent is the Connection with span_id 0"
    );
    assert_ne!(
        prop.parent_span_id, conn.parent_span_id,
        "property parent and root sentinel must be distinguishable"
    );
}

#[test]
fn end_with_no_begin_is_skipped() {
    let (mut rb, pid, sid, conv) = make_builder_ctx();
    {
        let mut b = NetSpanTreeBuilder::new(&mut rb, pid, sid, conv);
        // No prior Begin.
        b.on_connection_end(1, 10, 8).unwrap();
        b.finish();
    }
    let rows = collect_rows(rb);
    assert!(rows.is_empty());
}
