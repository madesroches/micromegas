use anyhow::{Context, Result};
use chrono::DateTime;
use datafusion::arrow::{
    array::{
        ArrayBuilder, BinaryBuilder, PrimitiveBuilder, StringBuilder, StringDictionaryBuilder,
    },
    datatypes::{DataType, Field, Int16Type, Int64Type, Schema, TimeUnit, TimestampNanosecondType},
    record_batch::RecordBatch,
};
use std::sync::Arc;

use crate::{metadata::ProcessMetadata, time::TimeRange};

pub fn images_table_schema() -> Schema {
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
        Field::new("name", DataType::Utf8, false),
        Field::new(
            "format",
            DataType::Dictionary(Box::new(DataType::Int16), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new("payload_size", DataType::Int64, false),
        Field::new("data", DataType::Binary, false),
    ])
}

pub struct ImagesRecordBuilder {
    process_ids: StringDictionaryBuilder<Int16Type>,
    stream_ids: StringDictionaryBuilder<Int16Type>,
    block_ids: StringDictionaryBuilder<Int16Type>,
    insert_times: PrimitiveBuilder<TimestampNanosecondType>,
    exes: StringDictionaryBuilder<Int16Type>,
    usernames: StringDictionaryBuilder<Int16Type>,
    computers: StringDictionaryBuilder<Int16Type>,
    times: PrimitiveBuilder<TimestampNanosecondType>,
    names: StringBuilder,
    formats: StringDictionaryBuilder<Int16Type>,
    payload_sizes: PrimitiveBuilder<Int64Type>,
    data: BinaryBuilder,
}

impl Default for ImagesRecordBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ImagesRecordBuilder {
    pub fn new() -> Self {
        Self {
            process_ids: StringDictionaryBuilder::new(),
            stream_ids: StringDictionaryBuilder::new(),
            block_ids: StringDictionaryBuilder::new(),
            insert_times: PrimitiveBuilder::new(),
            exes: StringDictionaryBuilder::new(),
            usernames: StringDictionaryBuilder::new(),
            computers: StringDictionaryBuilder::new(),
            times: PrimitiveBuilder::new(),
            names: StringBuilder::new(),
            formats: StringDictionaryBuilder::new(),
            payload_sizes: PrimitiveBuilder::new(),
            data: BinaryBuilder::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.times.len() == 0
    }

    pub fn get_time_range(&self) -> Option<TimeRange> {
        if self.is_empty() {
            return None;
        }
        let slice = self.times.values_slice();
        Some(TimeRange::new(
            DateTime::from_timestamp_nanos(slice[0]),
            DateTime::from_timestamp_nanos(slice[slice.len() - 1]),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn append(
        &mut self,
        process: &ProcessMetadata,
        process_id_str: &str,
        stream_id_str: &str,
        block_id_str: &str,
        insert_time_nanos: i64,
        time_ns: i64,
        name: &str,
        format: &str,
        payload_size: i64,
        image_data: &[u8],
    ) -> Result<()> {
        self.process_ids.append_value(process_id_str);
        self.stream_ids.append_value(stream_id_str);
        self.block_ids.append_value(block_id_str);
        self.insert_times.append_value(insert_time_nanos);
        self.exes.append_value(&process.exe);
        self.usernames.append_value(&process.username);
        self.computers.append_value(&process.computer);
        self.times.append_value(time_ns);
        self.names.append_value(name);
        self.formats.append_value(format);
        self.payload_sizes.append_value(payload_size);
        self.data.append_value(image_data);
        Ok(())
    }

    pub fn finish(mut self) -> Result<RecordBatch> {
        RecordBatch::try_new(
            Arc::new(images_table_schema()),
            vec![
                Arc::new(self.process_ids.finish()),
                Arc::new(self.stream_ids.finish()),
                Arc::new(self.block_ids.finish()),
                Arc::new(self.insert_times.finish().with_timezone_utc()),
                Arc::new(self.exes.finish()),
                Arc::new(self.usernames.finish()),
                Arc::new(self.computers.finish()),
                Arc::new(self.times.finish().with_timezone_utc()),
                Arc::new(self.names.finish()),
                Arc::new(self.formats.finish()),
                Arc::new(self.payload_sizes.finish()),
                Arc::new(self.data.finish()),
            ],
        )
        .with_context(|| "building images record batch")
    }
}
