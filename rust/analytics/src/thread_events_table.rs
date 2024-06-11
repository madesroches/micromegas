use anyhow::{Context, Result};
use datafusion::arrow::array::StringDictionaryBuilder;
use datafusion::arrow::datatypes::{DataType, Int64Type, TimeUnit};
use datafusion::arrow::{
    array::PrimitiveBuilder,
    datatypes::{Field, Int16Type, Int8Type, Schema, TimestampNanosecondType, UInt32Type},
    record_batch::RecordBatch,
};
use std::sync::Arc;

use crate::thread_block_processor::ThreadBlockProcessor;
use crate::time::ConvertTicks;

#[derive(Debug)]
pub struct ThreadEvent {
    pub id: i64,
    pub event_type: &'static str, // "begin" or "end"
    pub timestamp: i64,           // nanoseconds since jan 1st 1970 utc
    pub hash: u32,
    pub name: Arc<String>,
    pub target: Arc<String>,
    pub filename: Arc<String>,
    pub line: u32,
}

pub struct ThreadEventsRecordBuilder {
    begin_query_ns: i64,
    end_query_ns: i64,
    limit: i64,
    nb_rows: i64,
    convert_ticks: ConvertTicks,
    // data
    ids: PrimitiveBuilder<Int64Type>,
    event_types: StringDictionaryBuilder<Int8Type>,
    timestamps: PrimitiveBuilder<TimestampNanosecondType>,
    hashes: PrimitiveBuilder<UInt32Type>,
    names: StringDictionaryBuilder<Int16Type>,
    targets: StringDictionaryBuilder<Int16Type>,
    filenames: StringDictionaryBuilder<Int16Type>,
    lines: PrimitiveBuilder<UInt32Type>,
}

impl ThreadEventsRecordBuilder {
    pub fn new(
        begin_query_ns: i64,
        end_query_ns: i64,
        limit: i64,
        convert_ticks: ConvertTicks,
        capacity: usize,
    ) -> Self {
        Self {
            begin_query_ns,
            end_query_ns,
            limit,
            nb_rows: 0,
            convert_ticks,
            ids: PrimitiveBuilder::with_capacity(capacity),
            event_types: StringDictionaryBuilder::new(),
            timestamps: PrimitiveBuilder::with_capacity(capacity),
            hashes: PrimitiveBuilder::with_capacity(capacity),
            names: StringDictionaryBuilder::new(),
            targets: StringDictionaryBuilder::new(),
            filenames: StringDictionaryBuilder::new(),
            lines: PrimitiveBuilder::with_capacity(capacity),
        }
    }

    pub fn finish(mut self) -> Result<RecordBatch> {
        let schema = Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new(
                "event_type",
                DataType::Dictionary(Box::new(DataType::Int8), Box::new(DataType::Utf8)),
                false,
            ),
            Field::new(
                "timestamp",
                DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
                false,
            ),
            Field::new("hash", DataType::UInt32, false),
            Field::new(
                "name",
                DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
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
            Field::new("line", DataType::UInt32, false),
        ]);
        RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(self.ids.finish()),
                Arc::new(self.event_types.finish()),
                Arc::new(self.timestamps.finish().with_timezone_utc()),
                Arc::new(self.hashes.finish()),
                Arc::new(self.names.finish()),
                Arc::new(self.targets.finish()),
                Arc::new(self.filenames.finish()),
                Arc::new(self.lines.finish()),
            ],
        )
        .with_context(|| "building record batch")
    }

    fn process_event(
        &mut self,
        event_id: i64,
        event_type: &'static str,
        scope: crate::scope::ScopeDesc,
        ts: i64,
    ) -> Result<bool> {
        if self.nb_rows >= self.limit {
            return Ok(false);
        }
        let time = self.convert_ticks.ticks_to_nanoseconds(ts);
        if time < self.begin_query_ns {
            return Ok(true);
        }
        if time > self.end_query_ns {
            return Ok(false);
        }

        self.nb_rows += 1;
        self.ids.append_value(event_id);
        self.event_types.append_value(event_type);
        self.timestamps.append_value(time);
        self.hashes.append_value(scope.hash);
        self.names.append_value(&*scope.name);
        self.targets.append_value(&*scope.target);
        self.filenames.append_value(&*scope.filename);
        self.lines.append_value(scope.line);
        Ok(self.nb_rows < self.limit)
    }
}

impl ThreadBlockProcessor for ThreadEventsRecordBuilder {
    fn on_begin_thread_scope(
        &mut self,
        event_id: i64,
        scope: crate::scope::ScopeDesc,
        ts: i64,
    ) -> Result<bool> {
        self.process_event(event_id, "begin", scope, ts)
    }

    fn on_end_thread_scope(
        &mut self,
        event_id: i64,
        scope: crate::scope::ScopeDesc,
        ts: i64,
    ) -> Result<bool> {
        self.process_event(event_id, "end", scope, ts)
    }
}
