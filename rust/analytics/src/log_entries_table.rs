use std::sync::Arc;

use anyhow::{Context, Result};
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

pub struct LogEntriesRecordBuilder {
    pub times: PrimitiveBuilder<TimestampNanosecondType>,
    pub targets: StringDictionaryBuilder<Int16Type>,
    pub filenames: StringDictionaryBuilder<Int16Type>,
    pub lines: PrimitiveBuilder<Int32Type>,
    pub levels: PrimitiveBuilder<Int32Type>,
    pub msgs: StringBuilder,
}

impl LogEntriesRecordBuilder {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            times: PrimitiveBuilder::with_capacity(capacity),
            targets: StringDictionaryBuilder::new(),
            filenames: StringDictionaryBuilder::new(),
            lines: PrimitiveBuilder::with_capacity(capacity),
            levels: PrimitiveBuilder::with_capacity(capacity),
            msgs: StringBuilder::new(),
        }
    }

    pub fn append(&mut self, row: &LogEntry) -> Result<()> {
        self.times.append_value(row.time);
        self.targets.append_value(&*row.target);
        self.filenames.append_value(&*row.filename);
        self.lines.append_value(row.line);
        self.levels.append_value(row.level);
        self.msgs.append_value(&*row.msg);
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
                "filename",
                DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
                false,
            ),
            Field::new("line", DataType::Int32, false),
            Field::new("level", DataType::Int32, false),
            Field::new("msg", DataType::Utf8, false),
        ]);
        RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(self.times.finish().with_timezone_utc()),
                Arc::new(self.targets.finish()),
                Arc::new(self.filenames.finish()),
                Arc::new(self.lines.finish()),
                Arc::new(self.levels.finish()),
                Arc::new(self.msgs.finish()),
            ],
        )
        .with_context(|| "building record batch")
    }
}
