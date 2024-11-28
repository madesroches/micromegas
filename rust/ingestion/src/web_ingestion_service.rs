use crate::data_lake_connection::DataLakeConnection;
use anyhow::Context;
use anyhow::Result;
use bytes::Buf;
use micromegas_telemetry::block_wire_format;
use micromegas_telemetry::property::make_properties;
use micromegas_telemetry::stream_info::StreamInfo;
use micromegas_telemetry::wire_format::encode_cbor;
use micromegas_tracing::prelude::*;
use micromegas_tracing::property_set;

#[derive(Clone)]
pub struct WebIngestionService {
    lake: DataLakeConnection,
}

impl WebIngestionService {
    pub fn new(lake: DataLakeConnection) -> Self {
        Self { lake }
    }

    #[span_fn]
    pub async fn insert_block(&self, body: bytes::Bytes) -> Result<()> {
        let block: block_wire_format::Block = ciborium::from_reader(body.reader())
            .with_context(|| "parsing block_wire_format::Block")?;
        let encoded_payload = encode_cbor(&block.payload)?;
        let payload_size = encoded_payload.len();

        let process_id = &block.process_id;
        let stream_id = &block.stream_id;
        let block_id = &block.block_id;
        let obj_path = format!("blobs/{process_id}/{stream_id}/{block_id}");
        debug!("writing {obj_path}");

        use sqlx::types::chrono::{DateTime, FixedOffset};
        let begin_time = DateTime::<FixedOffset>::parse_from_rfc3339(&block.begin_time)
            .with_context(|| "parsing begin_time")?;
        let end_time = DateTime::<FixedOffset>::parse_from_rfc3339(&block.end_time)
            .with_context(|| "parsing end_time")?;
        let insert_time = sqlx::types::chrono::Utc::now();

        self.lake
            .blob_storage
            .put(&obj_path, encoded_payload.into())
            .await
            .with_context(|| "Error writing block to blob storage")?;

        debug!("recording block_id={block_id} stream_id={stream_id} process_id={process_id}");
        sqlx::query("INSERT INTO blocks VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11);")
            .bind(block_id)
            .bind(stream_id)
            .bind(process_id)
            .bind(begin_time)
            .bind(block.begin_ticks)
            .bind(end_time)
            .bind(block.end_ticks)
            .bind(block.nb_objects)
            .bind(block.object_offset)
            .bind(payload_size as i64)
            .bind(insert_time)
            .execute(&self.lake.db_pool)
            .await
            .with_context(|| "inserting into blocks")?;
        // this measure does not benefit from a dynamic property - I just want to make sure the feature works well
        // the cost in this context should be reasonnable
        imetric!(
            "payload_size_inserted",
            "bytes",
            property_set::PropertySet::find_or_create(vec![property_set::Property::new(
                "target",
                "micromegas::ingestion"
            ),]),
            payload_size as u64
        );
        debug!("recorded block_id={block_id} stream_id={stream_id} process_id={process_id}");

        Ok(())
    }

    #[span_fn]
    pub async fn insert_stream(&self, body: bytes::Bytes) -> Result<()> {
        let stream_info: StreamInfo =
            ciborium::from_reader(body.reader()).with_context(|| "parsing StreamInfo")?;
        info!(
            "new stream {} {:?} {:?}",
            stream_info.stream_id, &stream_info.tags, &stream_info.properties
        );
        sqlx::query("INSERT INTO streams VALUES($1,$2,$3,$4,$5,$6,$7);")
            .bind(stream_info.stream_id)
            .bind(stream_info.process_id)
            .bind(encode_cbor(&stream_info.dependencies_metadata)?)
            .bind(encode_cbor(&stream_info.objects_metadata)?)
            .bind(&stream_info.tags)
            .bind(make_properties(&stream_info.properties))
            .bind(sqlx::types::chrono::Utc::now())
            .execute(&self.lake.db_pool)
            .await
            .with_context(|| "inserting into streams")?;
        Ok(())
    }

    #[span_fn]
    pub async fn insert_process(&self, body: bytes::Bytes) -> Result<()> {
        let process_info: ProcessInfo =
            ciborium::from_reader(body.reader()).with_context(|| "parsing ProcessInfo")?;

        let insert_time = sqlx::types::chrono::Utc::now();
        sqlx::query("INSERT INTO processes VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13);")
            .bind(process_info.process_id)
            .bind(process_info.exe)
            .bind(process_info.username)
            .bind(process_info.realname)
            .bind(process_info.computer)
            .bind(process_info.distro)
            .bind(process_info.cpu_brand)
            .bind(process_info.tsc_frequency)
            .bind(process_info.start_time)
            .bind(process_info.start_ticks)
            .bind(insert_time)
            .bind(process_info.parent_process_id)
            .bind(make_properties(&process_info.properties))
            .execute(&self.lake.db_pool)
            .await
            .with_context(|| "executing sql insert into processes")?;
        Ok(())
    }
}
