use crate::measure::Measure;
use anyhow::{Context, Result};
use datafusion::arrow::{
    array::{PrimitiveBuilder, StringDictionaryBuilder},
    datatypes::{
        DataType, Field, Float64Type, Int16Type, Schema, TimeUnit, TimestampNanosecondType,
    },
    record_batch::RecordBatch,
};
use std::sync::Arc;

pub struct MetricsRecordBuilder {
    pub times: PrimitiveBuilder<TimestampNanosecondType>,
    pub targets: StringDictionaryBuilder<Int16Type>,
    pub names: StringDictionaryBuilder<Int16Type>,
    pub units: StringDictionaryBuilder<Int16Type>,
    pub values: PrimitiveBuilder<Float64Type>,
}

impl MetricsRecordBuilder {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            times: PrimitiveBuilder::with_capacity(capacity),
            targets: StringDictionaryBuilder::new(),
            names: StringDictionaryBuilder::new(),
            units: StringDictionaryBuilder::new(),
            values: PrimitiveBuilder::with_capacity(capacity),
        }
    }

    pub fn append(&mut self, row: &Measure) -> Result<()> {
        self.times.append_value(row.time);
        self.targets.append_value(&*row.target);
        self.names.append_value(&*row.name);
        self.units.append_value(&*row.unit);
        self.values.append_value(row.value);
        Ok(())
    }

    pub fn finish(mut self) -> Result<RecordBatch> {
        let schema = Schema::new(vec![
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
        ]);
        RecordBatch::try_new(
            Arc::new(schema),
            vec![
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
