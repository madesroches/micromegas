use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::DateTime;
use datafusion::arrow::array::ArrayBuilder;
use datafusion::arrow::array::BooleanBuilder;
use datafusion::arrow::array::PrimitiveBuilder;
use datafusion::arrow::array::StringDictionaryBuilder;
use datafusion::arrow::datatypes::DataType;
use datafusion::arrow::datatypes::Field;
use datafusion::arrow::datatypes::Int16Type;
use datafusion::arrow::datatypes::Int64Type;
use datafusion::arrow::datatypes::Schema;
use datafusion::arrow::datatypes::TimeUnit;
use datafusion::arrow::datatypes::TimestampNanosecondType;
use datafusion::arrow::datatypes::UInt32Type;
use datafusion::arrow::record_batch::RecordBatch;

use crate::time::TimeRange;

/// A single net span row (one per materialized span: Connection / Object / Property / RPC).
#[derive(Debug, Clone)]
pub struct NetSpanRecord {
    pub process_id: Arc<String>,
    pub stream_id: Arc<String>,
    pub span_id: i64,
    pub parent_span_id: i64,
    pub depth: u32,
    pub kind: Arc<String>,
    pub name: Arc<String>,
    pub connection_name: Arc<String>,
    pub is_outgoing: bool,
    pub begin_bits: i64,
    pub end_bits: i64,
    pub bit_size: i64,
    pub begin_time: i64,
    pub end_time: i64,
}

/// Returns the schema for the `net_spans` view.
pub fn net_spans_table_schema() -> Schema {
    Schema::new(vec![
        Field::new(
            "process_id",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new(
            "stream_id",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new("span_id", DataType::Int64, false),
        Field::new("parent_span_id", DataType::Int64, false),
        Field::new("depth", DataType::UInt32, false),
        Field::new(
            "kind",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new(
            "name",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new(
            "connection_name",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new("is_outgoing", DataType::Boolean, false),
        Field::new("begin_bits", DataType::Int64, false),
        Field::new("end_bits", DataType::Int64, false),
        Field::new("bit_size", DataType::Int64, false),
        Field::new(
            "begin_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new(
            "end_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
    ])
}

/// Accumulates `NetSpanRecord` rows into a single Arrow `RecordBatch`.
pub struct NetSpanRecordBuilder {
    process_ids: StringDictionaryBuilder<Int16Type>,
    stream_ids: StringDictionaryBuilder<Int16Type>,
    span_ids: PrimitiveBuilder<Int64Type>,
    parent_span_ids: PrimitiveBuilder<Int64Type>,
    depths: PrimitiveBuilder<UInt32Type>,
    kinds: StringDictionaryBuilder<Int16Type>,
    names: StringDictionaryBuilder<Int16Type>,
    connection_names: StringDictionaryBuilder<Int16Type>,
    is_outgoings: BooleanBuilder,
    begin_bits: PrimitiveBuilder<Int64Type>,
    end_bits: PrimitiveBuilder<Int64Type>,
    bit_sizes: PrimitiveBuilder<Int64Type>,
    begin_times: PrimitiveBuilder<TimestampNanosecondType>,
    end_times: PrimitiveBuilder<TimestampNanosecondType>,
    min_time: Option<i64>,
    max_time: Option<i64>,
}

impl NetSpanRecordBuilder {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            process_ids: StringDictionaryBuilder::new(),
            stream_ids: StringDictionaryBuilder::new(),
            span_ids: PrimitiveBuilder::with_capacity(capacity),
            parent_span_ids: PrimitiveBuilder::with_capacity(capacity),
            depths: PrimitiveBuilder::with_capacity(capacity),
            kinds: StringDictionaryBuilder::new(),
            names: StringDictionaryBuilder::new(),
            connection_names: StringDictionaryBuilder::new(),
            is_outgoings: BooleanBuilder::with_capacity(capacity),
            begin_bits: PrimitiveBuilder::with_capacity(capacity),
            end_bits: PrimitiveBuilder::with_capacity(capacity),
            bit_sizes: PrimitiveBuilder::with_capacity(capacity),
            begin_times: PrimitiveBuilder::with_capacity(capacity),
            end_times: PrimitiveBuilder::with_capacity(capacity),
            min_time: None,
            max_time: None,
        }
    }

    pub fn len(&self) -> i64 {
        self.span_ids.len() as i64
    }

    pub fn is_empty(&self) -> bool {
        self.span_ids.len() == 0
    }

    /// Returns the time range spanned by the rows accumulated so far.
    pub fn get_time_range(&self) -> Option<TimeRange> {
        match (self.min_time, self.max_time) {
            (Some(min_ns), Some(max_ns)) => Some(TimeRange::new(
                DateTime::from_timestamp_nanos(min_ns),
                DateTime::from_timestamp_nanos(max_ns),
            )),
            _ => None,
        }
    }

    pub fn append(&mut self, record: &NetSpanRecord) -> Result<()> {
        self.process_ids.append_value(&*record.process_id);
        self.stream_ids.append_value(&*record.stream_id);
        self.span_ids.append_value(record.span_id);
        self.parent_span_ids.append_value(record.parent_span_id);
        self.depths.append_value(record.depth);
        self.kinds.append_value(&*record.kind);
        self.names.append_value(&*record.name);
        self.connection_names.append_value(&*record.connection_name);
        self.is_outgoings.append_value(record.is_outgoing);
        self.begin_bits.append_value(record.begin_bits);
        self.end_bits.append_value(record.end_bits);
        self.bit_sizes.append_value(record.bit_size);
        self.begin_times.append_value(record.begin_time);
        self.end_times.append_value(record.end_time);
        self.min_time = Some(
            self.min_time
                .map(|m| m.min(record.begin_time))
                .unwrap_or(record.begin_time),
        );
        self.max_time = Some(
            self.max_time
                .map(|m| m.max(record.end_time))
                .unwrap_or(record.end_time),
        );
        Ok(())
    }

    pub fn finish(mut self) -> Result<RecordBatch> {
        RecordBatch::try_new(
            Arc::new(net_spans_table_schema()),
            vec![
                Arc::new(self.process_ids.finish()),
                Arc::new(self.stream_ids.finish()),
                Arc::new(self.span_ids.finish()),
                Arc::new(self.parent_span_ids.finish()),
                Arc::new(self.depths.finish()),
                Arc::new(self.kinds.finish()),
                Arc::new(self.names.finish()),
                Arc::new(self.connection_names.finish()),
                Arc::new(self.is_outgoings.finish()),
                Arc::new(self.begin_bits.finish()),
                Arc::new(self.end_bits.finish()),
                Arc::new(self.bit_sizes.finish()),
                Arc::new(self.begin_times.finish().with_timezone_utc()),
                Arc::new(self.end_times.finish().with_timezone_utc()),
            ],
        )
        .with_context(|| "building net spans record batch")
    }
}
