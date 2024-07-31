use crate::measure::Measure;
use anyhow::{Context, Result};
use datafusion::arrow::{
    array::{ArrayBuilder, PrimitiveBuilder, StringDictionaryBuilder},
    datatypes::{
        DataType, Field, Float64Type, Int16Type, Schema, TimeUnit, TimestampNanosecondType,
    },
    record_batch::RecordBatch,
};
use std::sync::Arc;

pub fn metrics_table_schema() -> Schema {
    Schema::new(vec![
        Field::new(
            "process_id",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
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
    ])
}

pub struct MetricsRecordBuilder {
    pub process_ids: StringDictionaryBuilder<Int16Type>,
    pub exes: StringDictionaryBuilder<Int16Type>,
    pub usernames: StringDictionaryBuilder<Int16Type>,
    pub computers: StringDictionaryBuilder<Int16Type>,
    pub times: PrimitiveBuilder<TimestampNanosecondType>,
    pub targets: StringDictionaryBuilder<Int16Type>,
    pub names: StringDictionaryBuilder<Int16Type>,
    pub units: StringDictionaryBuilder<Int16Type>,
    pub values: PrimitiveBuilder<Float64Type>,
}

impl MetricsRecordBuilder {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            process_ids: StringDictionaryBuilder::new(),
            exes: StringDictionaryBuilder::new(),
            usernames: StringDictionaryBuilder::new(),
            computers: StringDictionaryBuilder::new(),
            times: PrimitiveBuilder::with_capacity(capacity),
            targets: StringDictionaryBuilder::new(),
            names: StringDictionaryBuilder::new(),
            units: StringDictionaryBuilder::new(),
            values: PrimitiveBuilder::with_capacity(capacity),
        }
    }

    pub fn len(&self) -> i64 {
        self.times.len() as i64
    }

    pub fn is_empty(&self) -> bool {
        self.times.len() == 0
    }

    pub fn get_time_range(&self) -> Option<(i64, i64)> {
        if self.is_empty() {
            return None;
        }
        // assuming that the events are in order
        let slice = self.times.values_slice();
        Some((slice[0], slice[slice.len() - 1]))
    }

    pub fn append(&mut self, row: &Measure) -> Result<()> {
        self.process_ids
            .append_value(format!("{}", row.process.process_id));
        self.exes.append_value(&row.process.exe);
        self.usernames.append_value(&row.process.username);
        self.computers.append_value(&row.process.computer);
        self.times.append_value(row.time);
        self.targets.append_value(&*row.target);
        self.names.append_value(&*row.name);
        self.units.append_value(&*row.unit);
        self.values.append_value(row.value);
        Ok(())
    }

    pub fn finish(mut self) -> Result<RecordBatch> {
        RecordBatch::try_new(
            Arc::new(metrics_table_schema()),
            vec![
                Arc::new(self.process_ids.finish()),
                Arc::new(self.exes.finish()),
                Arc::new(self.usernames.finish()),
                Arc::new(self.computers.finish()),
                Arc::new(self.times.finish().with_timezone_utc()),
                Arc::new(self.targets.finish()),
                Arc::new(self.names.finish()),
                Arc::new(self.units.finish()),
                Arc::new(self.values.finish()),
            ],
        )
        .with_context(|| "building record batch")
    }
}
