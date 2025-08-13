use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::DateTime;
use datafusion::arrow::array::ArrayBuilder;
use datafusion::arrow::array::ListBuilder;
use datafusion::arrow::array::PrimitiveBuilder;
use datafusion::arrow::array::StringDictionaryBuilder;
use datafusion::arrow::array::StructBuilder;
use datafusion::arrow::datatypes::DataType;
use datafusion::arrow::datatypes::Field;
use datafusion::arrow::datatypes::Fields;
use datafusion::arrow::datatypes::Int16Type;
use datafusion::arrow::datatypes::Int32Type;
use datafusion::arrow::datatypes::Int64Type;
use datafusion::arrow::datatypes::Schema;
use datafusion::arrow::datatypes::TimeUnit;
use datafusion::arrow::datatypes::TimestampNanosecondType;
use datafusion::arrow::record_batch::RecordBatch;

use crate::arrow_properties::add_properties_to_builder;
use crate::time::TimeRange;

/// Represents a single async span event record.
#[derive(Debug, Clone)]
pub struct AsyncEventRecord {
    pub process_id: sqlx::types::Uuid,
    pub stream_id: Arc<String>,
    pub block_id: Arc<String>,
    pub insert_time: i64,
    pub exe: Arc<String>,
    pub username: Arc<String>,
    pub computer: Arc<String>,
    pub time: i64,
    pub event_type: Arc<String>,
    pub span_id: i64,
    pub parent_span_id: i64,
    pub name: Arc<String>,
    pub filename: Arc<String>,
    pub target: Arc<String>,
    pub line: u32,
    pub process_properties: std::collections::HashMap<String, String>,
}

/// Returns the schema for the async events table.
pub fn async_events_table_schema() -> Schema {
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
        Field::new(
            "block_id",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new(
            "insert_time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new(
            "exe",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new(
            "username",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new(
            "computer",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new(
            "time",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new(
            "event_type",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new("span_id", DataType::Int64, false),
        Field::new("parent_span_id", DataType::Int64, false),
        Field::new(
            "name",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new(
            "filename",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new(
            "target",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new("line", DataType::Int32, false),
        Field::new(
            "process_properties",
            DataType::List(Arc::new(Field::new(
                "Property",
                DataType::Struct(Fields::from(vec![
                    Field::new("key", DataType::Utf8, false),
                    Field::new("value", DataType::Utf8, false),
                ])),
                false,
            ))),
            false,
        ),
    ])
}

/// A builder for creating a `RecordBatch` of async event records.
pub struct AsyncEventRecordBuilder {
    process_ids: StringDictionaryBuilder<Int16Type>,
    stream_ids: StringDictionaryBuilder<Int16Type>,
    block_ids: StringDictionaryBuilder<Int16Type>,
    insert_times: PrimitiveBuilder<TimestampNanosecondType>,
    exes: StringDictionaryBuilder<Int16Type>,
    usernames: StringDictionaryBuilder<Int16Type>,
    computers: StringDictionaryBuilder<Int16Type>,
    times: PrimitiveBuilder<TimestampNanosecondType>,
    event_types: StringDictionaryBuilder<Int16Type>,
    span_ids: PrimitiveBuilder<Int64Type>,
    parent_span_ids: PrimitiveBuilder<Int64Type>,
    names: StringDictionaryBuilder<Int16Type>,
    filenames: StringDictionaryBuilder<Int16Type>,
    targets: StringDictionaryBuilder<Int16Type>,
    lines: PrimitiveBuilder<Int32Type>,
    process_properties: ListBuilder<StructBuilder>,
}

impl AsyncEventRecordBuilder {
    pub fn with_capacity(capacity: usize) -> Self {
        let prop_struct_fields = vec![
            Field::new("key", DataType::Utf8, false),
            Field::new("value", DataType::Utf8, false),
        ];
        let prop_field = Arc::new(Field::new(
            "Property",
            DataType::Struct(Fields::from(prop_struct_fields.clone())),
            false,
        ));
        let process_props_builder =
            ListBuilder::new(StructBuilder::from_fields(prop_struct_fields, capacity))
                .with_field(prop_field);

        Self {
            process_ids: StringDictionaryBuilder::new(),
            stream_ids: StringDictionaryBuilder::new(),
            block_ids: StringDictionaryBuilder::new(),
            insert_times: PrimitiveBuilder::with_capacity(capacity),
            exes: StringDictionaryBuilder::new(),
            usernames: StringDictionaryBuilder::new(),
            computers: StringDictionaryBuilder::new(),
            times: PrimitiveBuilder::with_capacity(capacity),
            event_types: StringDictionaryBuilder::new(),
            span_ids: PrimitiveBuilder::with_capacity(capacity),
            parent_span_ids: PrimitiveBuilder::with_capacity(capacity),
            names: StringDictionaryBuilder::new(),
            filenames: StringDictionaryBuilder::new(),
            targets: StringDictionaryBuilder::new(),
            lines: PrimitiveBuilder::with_capacity(capacity),
            process_properties: process_props_builder,
        }
    }

    pub fn get_time_range(&self) -> Option<TimeRange> {
        if self.is_empty() {
            return None;
        }
        // assuming that the events are in order
        let slice = self.times.values_slice();
        Some(TimeRange::new(
            DateTime::from_timestamp_nanos(slice[0]),
            DateTime::from_timestamp_nanos(slice[slice.len() - 1]),
        ))
    }

    pub fn len(&self) -> i64 {
        self.times.len() as i64
    }

    pub fn is_empty(&self) -> bool {
        self.times.len() == 0
    }

    pub fn append(&mut self, record: &AsyncEventRecord) -> Result<()> {
        self.process_ids
            .append_value(format!("{}", record.process_id));
        self.stream_ids.append_value(&*record.stream_id);
        self.block_ids.append_value(&*record.block_id);
        self.insert_times.append_value(record.insert_time);
        self.exes.append_value(&*record.exe);
        self.usernames.append_value(&*record.username);
        self.computers.append_value(&*record.computer);
        self.times.append_value(record.time);
        self.event_types.append_value(&*record.event_type);
        self.span_ids.append_value(record.span_id);
        self.parent_span_ids.append_value(record.parent_span_id);
        self.names.append_value(&*record.name);
        self.filenames.append_value(&*record.filename);
        self.targets.append_value(&*record.target);
        self.lines.append_value(record.line as i32);
        add_properties_to_builder(&record.process_properties, &mut self.process_properties)?;
        Ok(())
    }

    pub fn finish(mut self) -> Result<RecordBatch> {
        RecordBatch::try_new(
            Arc::new(async_events_table_schema()),
            vec![
                Arc::new(self.process_ids.finish()),
                Arc::new(self.stream_ids.finish()),
                Arc::new(self.block_ids.finish()),
                Arc::new(self.insert_times.finish().with_timezone_utc()),
                Arc::new(self.exes.finish()),
                Arc::new(self.usernames.finish()),
                Arc::new(self.computers.finish()),
                Arc::new(self.times.finish().with_timezone_utc()),
                Arc::new(self.event_types.finish()),
                Arc::new(self.span_ids.finish()),
                Arc::new(self.parent_span_ids.finish()),
                Arc::new(self.names.finish()),
                Arc::new(self.filenames.finish()),
                Arc::new(self.targets.finish()),
                Arc::new(self.lines.finish()),
                Arc::new(self.process_properties.finish()),
            ],
        )
        .with_context(|| "building record batch")
    }
}
