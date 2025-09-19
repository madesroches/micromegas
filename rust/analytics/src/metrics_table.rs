use crate::{
    arrow_properties::{add_properties_to_jsonb_builder, add_property_set_to_jsonb_builder},
    measure::Measure,
    time::TimeRange,
};
use anyhow::{Context, Result};
use chrono::DateTime;
use datafusion::arrow::{
    array::{ArrayBuilder, BinaryDictionaryBuilder, PrimitiveBuilder, StringDictionaryBuilder},
    datatypes::{
        DataType, Field, Float64Type, Int16Type, Int32Type, Schema, TimeUnit,
        TimestampNanosecondType,
    },
    record_batch::RecordBatch,
};
use std::sync::Arc;

/// Returns the schema for the metrics table.
pub fn metrics_table_schema() -> Schema {
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
        Field::new(
            "name",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new(
            "unit",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new("value", DataType::Float64, false),
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

/// A builder for creating a `RecordBatch` of metrics.
pub struct MetricsRecordBuilder {
    pub process_ids: StringDictionaryBuilder<Int16Type>,
    pub stream_ids: StringDictionaryBuilder<Int16Type>,
    pub block_ids: StringDictionaryBuilder<Int16Type>,
    pub insert_times: PrimitiveBuilder<TimestampNanosecondType>,
    pub exes: StringDictionaryBuilder<Int16Type>,
    pub usernames: StringDictionaryBuilder<Int16Type>,
    pub computers: StringDictionaryBuilder<Int16Type>,
    pub times: PrimitiveBuilder<TimestampNanosecondType>,
    pub targets: StringDictionaryBuilder<Int16Type>,
    pub names: StringDictionaryBuilder<Int16Type>,
    pub units: StringDictionaryBuilder<Int16Type>,
    pub values: PrimitiveBuilder<Float64Type>,
    pub properties: BinaryDictionaryBuilder<Int32Type>,
    pub process_properties: BinaryDictionaryBuilder<Int32Type>,
}

impl MetricsRecordBuilder {
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
            names: StringDictionaryBuilder::new(),
            units: StringDictionaryBuilder::new(),
            values: PrimitiveBuilder::with_capacity(capacity),
            properties: BinaryDictionaryBuilder::new(),
            process_properties: BinaryDictionaryBuilder::new(),
        }
    }

    pub fn len(&self) -> i64 {
        self.times.len() as i64
    }

    pub fn is_empty(&self) -> bool {
        self.times.len() == 0
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

    pub fn append(&mut self, row: &Measure) -> Result<()> {
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
        self.names.append_value(&*row.name);
        self.units.append_value(&*row.unit);
        self.values.append_value(row.value);
        add_property_set_to_jsonb_builder(&row.properties, &mut self.properties)?;
        add_properties_to_jsonb_builder(&row.process.properties, &mut self.process_properties)?;
        Ok(())
    }

    pub fn finish(mut self) -> Result<RecordBatch> {
        RecordBatch::try_new(
            Arc::new(metrics_table_schema()),
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
                Arc::new(self.names.finish()),
                Arc::new(self.units.finish()),
                Arc::new(self.values.finish()),
                Arc::new(self.properties.finish()),
                Arc::new(self.process_properties.finish()),
            ],
        )
        .with_context(|| "building record batch")
    }
}
