//! Arrow schema for the `otel_spans` view.
//!
//! Per the plan, `trace_id` is `FixedSizeBinary[16]` and `span_id` / `parent_span_id`
//! are `FixedSizeBinary[8]` — the lengths are fixed by W3C Trace Context, so the
//! variable-`Binary` offsets array would be pure overhead.

use datafusion::arrow::datatypes::{DataType, Field, Schema, TimeUnit};

pub fn otel_spans_table_schema() -> Schema {
    Schema::new(vec![
        Field::new(
            "process_id",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new(
            "stream_id",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new(
            "block_id",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new(
            "insert_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("exe", DataType::Utf8, false),
        Field::new("username", DataType::Utf8, false),
        Field::new("computer", DataType::Utf8, false),
        Field::new(
            "process_properties",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Binary)),
            false,
        ),
        Field::new("trace_id", DataType::FixedSizeBinary(16), false),
        Field::new("span_id", DataType::FixedSizeBinary(8), false),
        Field::new("parent_span_id", DataType::FixedSizeBinary(8), true),
        Field::new(
            "start_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new(
            "end_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("duration", DataType::Int64, false),
        Field::new(
            "name",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new(
            "kind",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new(
            "status",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new("status_message", DataType::Utf8, true),
        Field::new(
            "properties",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Binary)),
            false,
        ),
        Field::new("events", DataType::Binary, false),
        Field::new("links", DataType::Binary, false),
    ])
}
