use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::DateTime;
use datafusion::arrow::array::ArrayBuilder;
use datafusion::arrow::array::BinaryDictionaryBuilder;
use datafusion::arrow::array::PrimitiveBuilder;
use datafusion::arrow::array::StringBuilder;
use datafusion::arrow::array::StringDictionaryBuilder;
use datafusion::arrow::datatypes::DataType;
use datafusion::arrow::datatypes::Field;
use datafusion::arrow::datatypes::Int16Type;
use datafusion::arrow::datatypes::Int32Type;
use datafusion::arrow::datatypes::Schema;
use datafusion::arrow::datatypes::TimeUnit;
use datafusion::arrow::datatypes::TimestampNanosecondType;
use datafusion::arrow::record_batch::RecordBatch;

use crate::log_entry::LogEntry;
use crate::metadata::ProcessMetadata;
use crate::property_set_jsonb_dictionary_builder::PropertySetJsonbDictionaryBuilder;
use crate::time::TimeRange;

/// Returns the schema for the log entries table.
pub fn log_table_schema() -> Schema {
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
            "target",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new("level", DataType::Int32, false),
        Field::new("msg", DataType::Utf8, false),
        Field::new(
            "properties",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Binary)),
            false,
        ),
        Field::new(
            "process_properties",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Binary)),
            false,
        ),
    ])
}

/// A builder for creating a `RecordBatch` of log entries.
pub struct LogEntriesRecordBuilder {
    process_ids: StringDictionaryBuilder<Int16Type>,
    stream_ids: StringDictionaryBuilder<Int16Type>,
    block_ids: StringDictionaryBuilder<Int16Type>,
    insert_times: PrimitiveBuilder<TimestampNanosecondType>,
    exes: StringDictionaryBuilder<Int16Type>,
    usernames: StringDictionaryBuilder<Int16Type>,
    computers: StringDictionaryBuilder<Int16Type>,
    times: PrimitiveBuilder<TimestampNanosecondType>,
    targets: StringDictionaryBuilder<Int16Type>,
    levels: PrimitiveBuilder<Int32Type>,
    msgs: StringBuilder,
    properties: PropertySetJsonbDictionaryBuilder,
    process_properties: BinaryDictionaryBuilder<Int32Type>,
}

impl LogEntriesRecordBuilder {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            process_ids: StringDictionaryBuilder::new(),
            stream_ids: StringDictionaryBuilder::new(),
            block_ids: StringDictionaryBuilder::new(),
            insert_times: PrimitiveBuilder::with_capacity(capacity),
            exes: StringDictionaryBuilder::new(),
            usernames: StringDictionaryBuilder::new(),
            computers: StringDictionaryBuilder::new(),
            times: PrimitiveBuilder::with_capacity(capacity),
            targets: StringDictionaryBuilder::new(),
            levels: PrimitiveBuilder::with_capacity(capacity),
            msgs: StringBuilder::new(),
            properties: PropertySetJsonbDictionaryBuilder::new(capacity),
            process_properties: BinaryDictionaryBuilder::new(),
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

    pub fn append(&mut self, row: &LogEntry) -> Result<()> {
        self.process_ids
            .append_value(format!("{}", row.process.process_id));
        self.stream_ids.append_value(&*row.stream_id);
        self.block_ids.append_value(&*row.block_id);
        self.insert_times.append_value(row.insert_time);
        self.exes.append_value(&row.process.exe);
        self.usernames.append_value(&row.process.username);
        self.computers.append_value(&row.process.computer);
        self.times.append_value(row.time);
        self.targets.append_value(&*row.target);
        self.levels.append_value(row.level);
        self.msgs.append_value(&*row.msg);
        self.properties.append_property_set(&row.properties)?;
        self.process_properties
            .append_value(&*row.process.properties);
        Ok(())
    }

    /// Append only per-entry variable data (optimized for batch processing)
    pub fn append_entry_only(&mut self, row: &LogEntry) -> Result<()> {
        // Only append fields that truly vary per log entry
        self.times.append_value(row.time);
        self.targets.append_value(&*row.target);
        self.levels.append_value(row.level);
        self.msgs.append_value(&*row.msg);
        self.properties.append_property_set(&row.properties)?;
        Ok(())
    }

    /// Batch fill all constant columns for all entries in block
    pub fn fill_constant_columns(
        &mut self,
        process: &ProcessMetadata,
        stream_id: &str,
        block_id: &str,
        insert_time: i64,
        entry_count: usize,
    ) -> Result<()> {
        let process_id_str = format!("{}", process.process_id);

        // For PrimitiveBuilder (insert_times): use append_slice for better performance
        let insert_times_slice = vec![insert_time; entry_count];
        self.insert_times.append_slice(&insert_times_slice);

        self.process_ids.append_values(&process_id_str, entry_count);
        self.stream_ids.append_values(stream_id, entry_count);
        self.block_ids.append_values(block_id, entry_count);
        self.exes.append_values(&process.exe, entry_count);
        self.usernames.append_values(&process.username, entry_count);
        self.computers.append_values(&process.computer, entry_count);
        self.process_properties
            .append_values(&**process.properties, entry_count);

        Ok(())
    }

    pub fn finish(mut self) -> Result<RecordBatch> {
        RecordBatch::try_new(
            Arc::new(log_table_schema()),
            vec![
                Arc::new(self.process_ids.finish()),
                Arc::new(self.stream_ids.finish()),
                Arc::new(self.block_ids.finish()),
                Arc::new(self.insert_times.finish().with_timezone_utc()),
                Arc::new(self.exes.finish()),
                Arc::new(self.usernames.finish()),
                Arc::new(self.computers.finish()),
                Arc::new(self.times.finish().with_timezone_utc()),
                Arc::new(self.targets.finish()),
                Arc::new(self.levels.finish()),
                Arc::new(self.msgs.finish()),
                Arc::new(self.properties.finish()?),
                Arc::new(self.process_properties.finish()),
            ],
        )
        .with_context(|| "building record batch")
    }
}
