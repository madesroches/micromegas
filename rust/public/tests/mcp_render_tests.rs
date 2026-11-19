use arrow::array::{ArrayRef, Int32Array, ListArray, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Int32Type, Schema};
use futures::stream;
use micromegas::servers::mcp::render::{RenderLimits, render_stream, truncate_text};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn batch(start: i32, rows: i32) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![
        Field::new("n", DataType::Int32, false),
        Field::new("s", DataType::Utf8, true),
    ]));
    let ns = Int32Array::from_iter_values(start..start + rows);
    let ss = StringArray::from_iter((start..start + rows).map(|i| {
        if i % 2 == 0 {
            Some(format!("v{i}"))
        } else {
            None
        }
    }));
    RecordBatch::try_new(schema, vec![Arc::new(ns), Arc::new(ss)]).expect("batch")
}

fn limits(max_rows: usize, max_bytes: usize) -> RenderLimits {
    RenderLimits {
        max_rows,
        max_bytes,
    }
}

type Item = Result<RecordBatch, std::convert::Infallible>;

#[tokio::test]
async fn renders_a_table_with_nulls() {
    let s = stream::iter(vec![Ok(batch(0, 3)) as Item]);
    let r = render_stream(s, limits(100, 10_000)).await.expect("render");
    assert_eq!(r.rows, 3);
    assert!(!r.truncated);
    assert!(
        r.text
            .starts_with("| n | s |\n| --- | --- |\n| 0 | v0 |\n| 1 | NULL |")
    );
}

#[tokio::test]
async fn row_cap_truncates_and_stops_pulling() {
    let pulled = Arc::new(AtomicUsize::new(0));
    let counter = pulled.clone();
    let s = stream::iter((0..100).map(move |i| {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok(batch(i * 10, 10)) as Item
    }));
    let r = render_stream(s, limits(25, 1_000_000))
        .await
        .expect("render");
    assert_eq!(r.rows, 25);
    assert!(r.truncated);
    assert!(
        r.text
            .ends_with("truncated at 25 rows, add filters or aggregate\n")
    );
    assert_eq!(pulled.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn exact_row_cap_is_not_truncated_when_stream_ends() {
    let s = stream::iter(vec![Ok(batch(0, 5)) as Item]);
    let r = render_stream(s, limits(5, 10_000)).await.expect("render");
    assert!(!r.truncated);
    assert_eq!(r.rows, 5);
}

#[tokio::test]
async fn byte_cap_is_checked_per_row() {
    let s = stream::iter(vec![Ok(batch(0, 1000)) as Item]);
    let r = render_stream(s, limits(10_000, 200)).await.expect("render");
    assert!(r.truncated);
    assert!(r.rows > 0 && r.rows < 1000);
    assert!(r.text.contains("truncated at"));
    let table_len = r.text.split("\ntruncated").next().expect("table").len();
    assert!(table_len <= 200);
}

#[tokio::test]
async fn first_row_larger_than_cap_is_cut() {
    let schema = Arc::new(Schema::new(vec![Field::new("msg", DataType::Utf8, false)]));
    let big = StringArray::from(vec!["é".repeat(1000)]);
    let b = RecordBatch::try_new(schema, vec![Arc::new(big)]).expect("batch");
    let r = render_stream(stream::iter(vec![Ok(b) as Item]), limits(10, 100))
        .await
        .expect("render");
    assert_eq!(r.rows, 1);
    assert!(r.truncated);
    assert!(r.text.len() <= 100);
}

#[tokio::test]
async fn empty_result() {
    let r = render_stream(stream::iter(Vec::<Item>::new()), limits(10, 100))
        .await
        .expect("render");
    assert_eq!(r.text, "no data");
    let r = render_stream(stream::iter(vec![Ok(batch(0, 0)) as Item]), limits(10, 100))
        .await
        .expect("render");
    assert_eq!(r.text, "no data");
}

#[tokio::test]
async fn escapes_cells_and_renders_nested_types() {
    let list: ArrayRef = Arc::new(ListArray::from_iter_primitive::<Int32Type, _, _>(vec![
        Some(vec![Some(1), Some(2)]),
    ]));
    let text: ArrayRef = Arc::new(StringArray::from(vec!["a|b\nc"]));
    let schema = Arc::new(Schema::new(vec![
        Field::new("l", list.data_type().clone(), true),
        Field::new("t", DataType::Utf8, false),
    ]));
    let b = RecordBatch::try_new(schema, vec![list, text]).expect("batch");
    let r = render_stream(stream::iter(vec![Ok(b) as Item]), limits(10, 10_000))
        .await
        .expect("render");
    assert!(r.text.contains("[1, 2]"));
    assert!(r.text.contains("a\\|b\\nc"));
}

#[tokio::test]
async fn propagates_stream_errors() {
    let s = stream::iter(vec![Ok(batch(0, 2)), Err("boom")]);
    let err = render_stream(s, limits(100, 10_000))
        .await
        .expect_err("error");
    assert_eq!(err, "boom");
}

#[test]
fn truncate_text_notes_the_cut() {
    assert_eq!(truncate_text("abc", 10), "abc");
    assert!(truncate_text("abcdef", 3).starts_with("abc\n... truncated at 3 bytes"));
}
