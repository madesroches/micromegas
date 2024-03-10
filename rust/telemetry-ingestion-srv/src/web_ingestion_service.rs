use crate::data_lake_connection::DataLakeConnection;
use anyhow::Context;
use anyhow::Result;
use bytes::Buf;
use telemetry_sink::block_wire_format;
use telemetry_sink::stream_info::StreamInfo;
use telemetry_sink::wire_format::encode_cbor;
use tracing::prelude::*;

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
        let block: block_wire_format::Block = ciborium::from_reader(body.reader())?;
        let mut connection = self.lake.db_pool.acquire().await?;
        let encoded_payload = encode_cbor(&block.payload)?;
        let payload_size = encoded_payload.len();
        self.lake
            .blob_storage
            .write_blob(&block.block_id, &encoded_payload)
            .await
            .with_context(|| "Error writing block to blob storage")?;

        sqlx::query("INSERT INTO blocks VALUES(?,?,?,?,?,?,?,?);")
            .bind(block.block_id)
            .bind(block.stream_id)
            .bind(block.begin_time)
            .bind(block.begin_ticks)
            .bind(block.end_time)
            .bind(block.end_ticks)
            .bind(block.nb_objects)
            .bind(payload_size as i64)
            .execute(&mut connection)
            .await
            .with_context(|| "inserting into blocks")?;

        Ok(())
    }

    #[span_fn]
    pub async fn insert_stream(&self, stream_info: StreamInfo) -> Result<()> {
        info!(
            "new stream {:?} {}",
            &stream_info.tags, stream_info.stream_id
        );
        let mut connection = self.lake.db_pool.acquire().await?;
        sqlx::query("INSERT INTO streams VALUES(?,?,?,?,?,?);")
            .bind(stream_info.stream_id)
            .bind(stream_info.process_id)
            .bind(encode_cbor(&stream_info.dependencies_metadata)?)
            .bind(encode_cbor(&stream_info.objects_metadata)?)
            .bind(serde_json::to_string(&stream_info.tags)?)
            .bind(serde_json::to_string(&stream_info.properties)?)
            .execute(&mut connection)
            .await
            .with_context(|| "inserting into streams")?;
        Ok(())
    }

    #[span_fn]
    pub async fn insert_process(&self, body: serde_json::value::Value) -> Result<()> {
        let mut connection = self.lake.db_pool.acquire().await?;
        let current_date: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
        info!("insert_process: {body:?}");
        let tsc_frequency = body["tsc_frequency"]
            .as_str()
            .with_context(|| "reading field tsc_frequency")?
            .parse::<i64>()
            .with_context(|| "parsing tsc_frequency")?;

        let start_ticks = body["start_ticks"]
            .as_str()
            .with_context(|| "reading field start_ticks")?
            .parse::<i64>()
            .with_context(|| "parsing start_ticks")?;

        sqlx::query("INSERT INTO processes VALUES(?,?,?,?,?,?,?,?,?,?,?,?);")
            .bind(
                body["process_id"]
                    .as_str()
                    .with_context(|| "reading field process_id")?,
            )
            .bind(body["exe"].as_str().with_context(|| "reading field exe")?)
            .bind(
                body["username"]
                    .as_str()
                    .with_context(|| "reading field username")?,
            )
            .bind(
                body["realname"]
                    .as_str()
                    .with_context(|| "reading field realname")?,
            )
            .bind(
                body["computer"]
                    .as_str()
                    .with_context(|| "reading field computer")?,
            )
            .bind(
                body["distro"]
                    .as_str()
                    .with_context(|| "reading field distro")?,
            )
            .bind(
                body["cpu_brand"]
                    .as_str()
                    .with_context(|| "reading field cpu_brand")?,
            )
            .bind(tsc_frequency)
            .bind(
                body["start_time"]
                    .as_str()
                    .with_context(|| "reading field start_time")?,
            )
            .bind(start_ticks)
            .bind(current_date.format("%Y-%m-%d").to_string())
            .bind(
                body["parent_process_id"]
                    .as_str()
                    .with_context(|| "reading field parent_process_id")?,
            )
            .execute(&mut connection)
            .await
            .with_context(|| "executing sql insert into processes")?;
        Ok(())
    }
}
